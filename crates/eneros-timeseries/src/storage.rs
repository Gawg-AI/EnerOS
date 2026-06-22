use eneros_core::ElementId;
use super::engine::DataPoint;
use super::gorilla::{GorillaDecoder, GorillaEncoder};

/// Time-series storage abstraction
pub trait TimeSeriesStorage: Send + Sync {
    /// Store a data point
    fn store(
        &self,
        element_id: ElementId,
        parameter: &str,
        point: DataPoint,
    ) -> Result<(), String>;

    /// Retrieve data points
    fn retrieve(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<DataPoint>, String>;

    /// Get latest data point
    fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<Option<DataPoint>, String>;

    /// Delete old data
    fn cleanup(&self, before: i64) -> Result<usize, String>;
}

/// In-memory storage implementation
pub struct InMemoryStorage {
    data: std::sync::RwLock<std::collections::HashMap<(ElementId, String), Vec<DataPoint>>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            data: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeSeriesStorage for InMemoryStorage {
    fn store(
        &self,
        element_id: ElementId,
        parameter: &str,
        point: DataPoint,
    ) -> Result<(), String> {
        let mut data = self.data.write().map_err(|e| e.to_string())?;
        let key = (element_id, parameter.to_string());
        data.entry(key).or_default().push(point);
        Ok(())
    }

    fn retrieve(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<DataPoint>, String> {
        let data = self.data.read().map_err(|e| e.to_string())?;
        let key = (element_id, parameter.to_string());

        Ok(data
            .get(&key)
            .map(|points| {
                points
                    .iter()
                    .filter(|p| {
                        let ts = p.timestamp.timestamp_millis();
                        ts >= start && ts <= end
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<Option<DataPoint>, String> {
        let data = self.data.read().map_err(|e| e.to_string())?;
        let key = (element_id, parameter.to_string());

        Ok(data.get(&key).and_then(|points| points.last().cloned()))
    }

    fn cleanup(&self, before: i64) -> Result<usize, String> {
        let mut data = self.data.write().map_err(|e| e.to_string())?;
        let mut removed = 0;

        for points in data.values_mut() {
            let original_len = points.len();
            points.retain(|p| p.timestamp.timestamp_millis() >= before);
            removed += original_len - points.len();
        }

        Ok(removed)
    }
}

// =====================================================================
// CompressedStorage — Gorilla 压缩存储后端（T029-13）
// =====================================================================

/// 一个 Gorilla 压缩数据块
///
/// 每个 block 封装最多 `block_size` 个连续数据点，按时间戳升序排列。
/// block 一旦密封（seal）即不可变，查询时按需解码。
#[derive(Debug, Clone)]
struct GorillaBlock {
    /// 块内首点时间戳（毫秒）
    start_ts: i64,
    /// 块内末点时间戳（毫秒）
    end_ts: i64,
    /// 块内数据点数
    count: u32,
    /// Gorilla 压缩字节流
    compressed: Vec<u8>,
}

impl GorillaBlock {
    /// 将一批数据点编码为一个密封的 Gorilla 块
    fn seal(points: &[DataPoint]) -> Self {
        debug_assert!(!points.is_empty());
        let mut enc = GorillaEncoder::new();
        for p in points {
            enc.encode(
                p.timestamp.timestamp_millis(),
                p.value,
                &p.quality,
            );
        }
        let start_ts = points
            .first()
            .map(|p| p.timestamp.timestamp_millis())
            .unwrap_or(0);
        let end_ts = points
            .last()
            .map(|p| p.timestamp.timestamp_millis())
            .unwrap_or(0);
        Self {
            start_ts,
            end_ts,
            count: enc.count(),
            compressed: enc.finish(),
        }
    }

    /// 解码块内全部数据点
    fn decode(&self) -> Vec<DataPoint> {
        let mut dec = GorillaDecoder::new(&self.compressed);
        let mut out = Vec::with_capacity(self.count as usize);
        while let Some((ts_ms, value, quality)) = dec.next() {
            let timestamp = chrono::DateTime::from_timestamp_millis(ts_ms)
                .unwrap_or_else(chrono::Utc::now);
            out.push(DataPoint {
                timestamp,
                value,
                quality,
            });
        }
        out
    }

    /// 块的时间范围是否与查询范围 [start, end]（毫秒）相交
    fn overlaps(&self, start: i64, end: i64) -> bool {
        self.end_ts >= start && self.start_ts <= end
    }
}

/// Gorilla 压缩存储后端
///
/// 实现 [`TimeSeriesStorage`] trait，将时序数据按块进行 Gorilla 压缩后
/// 在内存中保存。写入时点先缓冲到 pending 区，达到 `block_size` 后
/// 压缩为一个 [`GorillaBlock`] 并密封。查询时仅解码与查询时间范围相交
/// 的块，并合并 pending 区中尚未压缩的点。
///
/// # 压缩比
///
/// 对于典型的电力时序数据（固定间隔采样 + 缓慢变化的浮点值），
/// Gorilla 压缩通常达到 5x–15x 压缩比。
///
/// # 查询延迟
///
/// 块按需解码，单块解码 < 1ms（1000 点），查询 10 万点 < 50ms。
pub struct CompressedStorage {
    /// 已压缩的块：每个 (element_id, parameter) 键对应一组按时间排序的块
    blocks: std::sync::RwLock<
        std::collections::HashMap<(ElementId, String), Vec<GorillaBlock>>,
    >,
    /// 待压缩的缓冲区：每个键对应一组尚未达到 block_size 的点
    pending: std::sync::RwLock<
        std::collections::HashMap<(ElementId, String), Vec<DataPoint>>,
    >,
    /// 每个压缩块包含的数据点数
    block_size: usize,
}

impl CompressedStorage {
    /// 创建一个新的 Gorilla 压缩存储后端
    ///
    /// `block_size` 控制每个压缩块包含的数据点数。较大的值通常带来
    /// 更好的压缩比，但增加单块解码延迟。默认 1000 适合大多数场景。
    pub fn new(block_size: usize) -> Self {
        Self {
            blocks: std::sync::RwLock::new(std::collections::HashMap::new()),
            pending: std::sync::RwLock::new(std::collections::HashMap::new()),
            block_size: block_size.max(1),
        }
    }

    /// 将 pending 区中达到 block_size 的点压缩为块
    fn maybe_seal_block(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<(), String> {
        let to_seal: Option<Vec<DataPoint>> = {
            let mut pending = self.pending.write().map_err(|e| e.to_string())?;
            let key = (element_id, parameter.to_string());
            if let Some(buf) = pending.get_mut(&key) {
                if buf.len() >= self.block_size {
                    // 取出 block_size 个点进行压缩
                    let drained: Vec<DataPoint> = buf.drain(..self.block_size).collect();
                    Some(drained)
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(points) = to_seal {
            let block = GorillaBlock::seal(&points);
            let mut blocks = self.blocks.write().map_err(|e| e.to_string())?;
            let key = (element_id, parameter.to_string());
            blocks.entry(key).or_default().push(block);
        }

        Ok(())
    }

    /// 压缩比统计：原始字节数 / 压缩后字节数
    ///
    /// 原始字节数 = 所有点 × (8 + 8 + 1) 字节（timestamp + value + quality）
    /// 压缩后字节数 = 所有 GorillaBlock.compressed 的字节数之和
    /// （pending 区的点不计入压缩比统计）
    pub fn compression_ratio(&self) -> f64 {
        let blocks = self.blocks.read().unwrap();
        let mut original_bytes: usize = 0;
        let mut compressed_bytes: usize = 0;
        for blks in blocks.values() {
            for b in blks {
                original_bytes += b.count as usize * (8 + 8 + 1);
                compressed_bytes += b.compressed.len();
            }
        }
        if compressed_bytes == 0 {
            return 0.0;
        }
        original_bytes as f64 / compressed_bytes as f64
    }

    /// 已压缩的数据点总数
    pub fn compressed_point_count(&self) -> u64 {
        let blocks = self.blocks.read().unwrap();
        blocks
            .values()
            .flat_map(|blks| blks.iter().map(|b| b.count as u64))
            .sum()
    }

    /// 已压缩的块总数
    pub fn block_count(&self) -> usize {
        let blocks = self.blocks.read().unwrap();
        blocks.values().map(|blks| blks.len()).sum()
    }
}

impl Default for CompressedStorage {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl TimeSeriesStorage for CompressedStorage {
    fn store(
        &self,
        element_id: ElementId,
        parameter: &str,
        point: DataPoint,
    ) -> Result<(), String> {
        // 1. 追加到 pending 缓冲区
        {
            let mut pending = self.pending.write().map_err(|e| e.to_string())?;
            let key = (element_id, parameter.to_string());
            pending.entry(key).or_default().push(point);
        }
        // 2. 如果 pending 达到 block_size，压缩为一个块
        self.maybe_seal_block(element_id, parameter)?;
        Ok(())
    }

    fn retrieve(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<DataPoint>, String> {
        let mut result = Vec::new();
        let key = (element_id, parameter.to_string());

        // 1. 解码与查询范围相交的已压缩块
        {
            let blocks = self.blocks.read().map_err(|e| e.to_string())?;
            if let Some(blks) = blocks.get(&key) {
                for b in blks {
                    if b.overlaps(start, end) {
                        let decoded = b.decode();
                        for p in decoded {
                            let ts = p.timestamp.timestamp_millis();
                            if ts >= start && ts <= end {
                                result.push(p);
                            }
                        }
                    }
                }
            }
        }

        // 2. 合并 pending 区中尚未压缩的点
        {
            let pending = self.pending.read().map_err(|e| e.to_string())?;
            if let Some(buf) = pending.get(&key) {
                for p in buf {
                    let ts = p.timestamp.timestamp_millis();
                    if ts >= start && ts <= end {
                        result.push(p.clone());
                    }
                }
            }
        }

        // 3. 按时间戳排序（块间 + pending 可能乱序）
        result.sort_by_key(|p| p.timestamp);
        Ok(result)
    }

    fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<Option<DataPoint>, String> {
        let key = (element_id, parameter.to_string());

        // 优先从 pending 区取末点（最新写入的点在 pending 末尾）
        {
            let pending = self.pending.read().map_err(|e| e.to_string())?;
            if let Some(buf) = pending.get(&key) {
                if let Some(last) = buf.last() {
                    return Ok(Some(last.clone()));
                }
            }
        }

        // pending 为空时，取最后一个块的末点
        let blocks = self.blocks.read().map_err(|e| e.to_string())?;
        if let Some(blks) = blocks.get(&key) {
            if let Some(last_block) = blks.last() {
                let decoded = last_block.decode();
                if let Some(last) = decoded.last() {
                    return Ok(Some(last.clone()));
                }
            }
        }

        Ok(None)
    }

    fn cleanup(&self, before: i64) -> Result<usize, String> {
        let mut removed = 0usize;

        // 1. 清理 pending 区
        {
            let mut pending = self.pending.write().map_err(|e| e.to_string())?;
            for buf in pending.values_mut() {
                let before_len = buf.len();
                buf.retain(|p| p.timestamp.timestamp_millis() >= before);
                removed += before_len - buf.len();
            }
        }

        // 2. 清理已压缩块：完全在 before 之前的块整体删除，
        //    跨越 before 的块解码后保留有效部分（重新压缩）
        {
            let mut blocks = self.blocks.write().map_err(|e| e.to_string())?;
            for blks in blocks.values_mut() {
                let mut new_blocks: Vec<GorillaBlock> = Vec::with_capacity(blks.len());
                for b in blks.drain(..) {
                    if b.end_ts < before {
                        // 整块过期
                        removed += b.count as usize;
                    } else if b.start_ts >= before {
                        // 整块保留
                        new_blocks.push(b);
                    } else {
                        // 跨越边界：解码、过滤、重新压缩
                        let decoded = b.decode();
                        let kept: Vec<DataPoint> = decoded
                            .into_iter()
                            .filter(|p| p.timestamp.timestamp_millis() >= before)
                            .collect();
                        removed += b.count as usize - kept.len();
                        if !kept.is_empty() {
                            new_blocks.push(GorillaBlock::seal(&kept));
                        }
                    }
                }
                *blks = new_blocks;
            }
        }

        Ok(removed)
    }
}

#[cfg(test)]
mod compressed_storage_tests {
    use super::*;
    use crate::engine::DataQuality;
    use chrono::{TimeZone, Utc};

    fn make_point(ts_secs: i64, value: f64, quality: DataQuality) -> DataPoint {
        DataPoint {
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            value,
            quality,
        }
    }

    #[test]
    fn test_compressed_storage_store_and_retrieve() {
        let storage = CompressedStorage::new(10);
        let base = Utc.timestamp_opt(1700000000, 0).unwrap();

        // 写入 5 个点（< block_size，全部留在 pending）
        for i in 0..5 {
            storage
                .store(1, "voltage", make_point(1700000000 + i, i as f64, DataQuality::Good))
                .unwrap();
        }

        let start = (base - chrono::Duration::hours(1)).timestamp_millis();
        let end = (base + chrono::Duration::hours(1)).timestamp_millis();
        let results = storage.retrieve(1, "voltage", start, end).unwrap();
        assert_eq!(results.len(), 5);
        for (i, p) in results.iter().enumerate() {
            assert_eq!(p.value, i as f64);
        }
    }

    #[test]
    fn test_compressed_storage_block_sealing() {
        let storage = CompressedStorage::new(10);

        // 写入 15 个点 → 应产生 1 个密封块（10 点）+ 5 个 pending 点
        for i in 0..15 {
            storage
                .store(1, "current", make_point(1700000000 + i, i as f64 * 2.0, DataQuality::Good))
                .unwrap();
        }

        assert_eq!(storage.block_count(), 1, "应密封 1 个块");
        assert_eq!(storage.compressed_point_count(), 10, "块内 10 个点");

        // 查询全部
        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700000100, 0).unwrap();
        let results = storage
            .retrieve(1, "current", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert_eq!(results.len(), 15, "应返回 15 个点（10 压缩 + 5 pending）");

        // 验证值正确
        for (i, p) in results.iter().enumerate() {
            assert_eq!(p.value, i as f64 * 2.0, "值不匹配 @ {}", i);
        }
    }

    #[test]
    fn test_compressed_storage_range_query() {
        let storage = CompressedStorage::new(5);

        // 写入 20 个点 → 4 个密封块
        for i in 0..20 {
            storage
                .store(1, "power", make_point(1700000000 + i, i as f64, DataQuality::Good))
                .unwrap();
        }
        assert_eq!(storage.block_count(), 4);

        // 查询 [1700000005, 1700000014] → 应返回 10 个点
        let start = Utc.timestamp_opt(1700000005, 0).unwrap();
        let end = Utc.timestamp_opt(1700000014, 0).unwrap();
        let results = storage
            .retrieve(1, "power", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert_eq!(results.len(), 10);
        assert_eq!(results[0].value, 5.0);
        assert_eq!(results[9].value, 14.0);
    }

    #[test]
    fn test_compressed_storage_latest() {
        let storage = CompressedStorage::new(5);

        // 写入 12 个点 → 2 个块 + 2 个 pending
        for i in 0..12 {
            storage
                .store(1, "load", make_point(1700000000 + i, i as f64, DataQuality::Good))
                .unwrap();
        }

        let latest = storage.latest(1, "load").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().value, 11.0);
    }

    #[test]
    fn test_compressed_storage_latest_from_block() {
        let storage = CompressedStorage::new(5);

        // 写入恰好 5 个点 → 1 个块，pending 为空
        for i in 0..5 {
            storage
                .store(1, "freq", make_point(1700000000 + i, 50.0 + i as f64, DataQuality::Good))
                .unwrap();
        }
        assert_eq!(storage.block_count(), 1);

        let latest = storage.latest(1, "freq").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().value, 54.0);
    }

    #[test]
    fn test_compressed_storage_cleanup() {
        let storage = CompressedStorage::new(5);

        // 写入 20 个点 → 4 个块
        for i in 0..20 {
            storage
                .store(1, "voltage", make_point(1700000000 + i, i as f64, DataQuality::Good))
                .unwrap();
        }
        assert_eq!(storage.block_count(), 4);

        // 清理 1700000010 之前的数据
        let cutoff = Utc.timestamp_opt(1700000010, 0).unwrap();
        let removed = storage.cleanup(cutoff.timestamp_millis()).unwrap();
        // 应删除 10 个点（前 2 个块）
        assert_eq!(removed, 10);

        // 验证剩余数据
        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700000100, 0).unwrap();
        let results = storage
            .retrieve(1, "voltage", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert_eq!(results.len(), 10);
        assert_eq!(results[0].value, 10.0);
        assert_eq!(results[9].value, 19.0);
    }

    #[test]
    fn test_compressed_storage_cleanup_partial_block() {
        let storage = CompressedStorage::new(10);

        // 写入 25 个点 → 2 个块 + 5 个 pending
        for i in 0..25 {
            storage
                .store(1, "power", make_point(1700000000 + i, i as f64, DataQuality::Good))
                .unwrap();
        }
        assert_eq!(storage.block_count(), 2);

        // 清理 1700000005 之前的数据 → 第 1 个块部分过期
        let cutoff = Utc.timestamp_opt(1700000005, 0).unwrap();
        let removed = storage.cleanup(cutoff.timestamp_millis()).unwrap();
        assert_eq!(removed, 5);

        // 验证剩余数据
        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700000100, 0).unwrap();
        let results = storage
            .retrieve(1, "power", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert_eq!(results.len(), 20);
        assert_eq!(results[0].value, 5.0);
        assert_eq!(results[19].value, 24.0);
    }

    #[test]
    fn test_compressed_storage_compression_ratio() {
        let storage = CompressedStorage::new(1000);

        // 写入 5000 个点，模拟真实电力电压数据
        // 关键：对采样值进行量化（0.01V 精度），使相邻样本经常相同 → XOR=0 → 1 bit
        // 这是工业电力数据的真实模式（ADC 量化 + 缓慢变化的物理量）
        for i in 0..5000 {
            let t = i as f64 * 0.001;
            let raw = 220.0 + (t * 0.5).sin() * 0.3 + (t * 2.0).sin() * 0.1;
            let v = (raw * 100.0).round() / 100.0; // 量化到 0.01V
            storage
                .store(1, "voltage", make_point(1700000000 + i, v, DataQuality::Good))
                .unwrap();
        }

        assert_eq!(storage.block_count(), 5);
        let ratio = storage.compression_ratio();
        eprintln!("CompressedStorage 压缩比: {:.2}x", ratio);
        assert!(ratio > 5.0, "压缩比应 > 5x，实际 {:.2}x", ratio);

        // 验证数据正确性
        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700010000, 0).unwrap();
        let results = storage
            .retrieve(1, "voltage", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert_eq!(results.len(), 5000);
    }

    #[test]
    fn test_compressed_storage_query_latency() {
        let storage = CompressedStorage::new(1000);

        // 写入 100,000 个点 → 100 个块
        // 使用量化数据（0.01V 精度）模拟真实电力数据
        for i in 0..100_000 {
            let t = i as f64 * 0.001;
            let raw = 220.0 + (t * 0.5).sin() * 0.3;
            let v = (raw * 100.0).round() / 100.0; // 量化到 0.01V
            storage
                .store(1, "voltage", make_point(1700000000 + i, v, DataQuality::Good))
                .unwrap();
        }
        assert_eq!(storage.block_count(), 100);

        // 查询全部 100,000 个点，测量延迟
        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700100000, 0).unwrap();
        let t0 = std::time::Instant::now();
        let results = storage
            .retrieve(1, "voltage", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        let elapsed = t0.elapsed();

        assert_eq!(results.len(), 100_000);
        eprintln!(
            "查询 100,000 个点耗时: {:.2}ms",
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(
            elapsed.as_millis() < 50,
            "查询延迟应 < 50ms，实际 {}ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn test_compressed_storage_multiple_keys() {
        let storage = CompressedStorage::new(5);

        // 两个不同的 (element_id, parameter) 键
        for i in 0..10 {
            storage
                .store(1, "voltage", make_point(1700000000 + i, i as f64, DataQuality::Good))
                .unwrap();
            storage
                .store(2, "current", make_point(1700000000 + i, i as f64 * 2.0, DataQuality::Good))
                .unwrap();
        }

        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700000100, 0).unwrap();

        let r1 = storage
            .retrieve(1, "voltage", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert_eq!(r1.len(), 10);

        let r2 = storage
            .retrieve(2, "current", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert_eq!(r2.len(), 10);
        assert_eq!(r2[5].value, 10.0);
    }

    #[test]
    fn test_compressed_storage_quality_round_trip() {
        let storage = CompressedStorage::new(5);

        // 混合质量码
        for i in 0..15 {
            let q = match i % 3 {
                0 => DataQuality::Good,
                1 => DataQuality::Uncertain,
                _ => DataQuality::Bad,
            };
            storage
                .store(1, "mixed", make_point(1700000000 + i, i as f64, q))
                .unwrap();
        }

        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700000100, 0).unwrap();
        let results = storage
            .retrieve(1, "mixed", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();

        assert_eq!(results.len(), 15);
        for (i, p) in results.iter().enumerate() {
            let expected_q = match i % 3 {
                0 => DataQuality::Good,
                1 => DataQuality::Uncertain,
                _ => DataQuality::Bad,
            };
            assert_eq!(p.quality, expected_q, "质量码不匹配 @ {}", i);
        }
    }

    #[test]
    fn test_compressed_storage_empty_query() {
        let storage = CompressedStorage::new(10);

        let start = Utc.timestamp_opt(1700000000, 0).unwrap();
        let end = Utc.timestamp_opt(1700000100, 0).unwrap();
        let results = storage
            .retrieve(99, "nonexistent", start.timestamp_millis(), end.timestamp_millis())
            .unwrap();
        assert!(results.is_empty());

        let latest = storage.latest(99, "nonexistent").unwrap();
        assert!(latest.is_none());
    }

    #[test]
    fn test_compressed_storage_default_block_size() {
        let storage = CompressedStorage::default();
        assert_eq!(storage.block_size, 1000);
    }
}
