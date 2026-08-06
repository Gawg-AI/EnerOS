//! 崩溃转储二进制格式（OBS-007）。
//!
//! 紧凑定长头 + 变长 payload + CRC32 校验。所有多字节字段为小端。
//!
//! ```text
//! Offset  Size  Field
//! 0       4     magic = b"ENCD"
//! 4       2     version = 1
//! 6       1     arch (1=aarch64, 0=unknown)
//! 7       1     build_id_len (0..=32)
//! 8       32    build_id (zero-padded)
//! 40      8     crash_ts_ns (0 = 未知)
//! 48      8     thread_id
//! 56      8     pc
//! 64      8     sp
//! 72      8     lr
//! 80      2     panic_msg_len (0..=512)
//! 82      2     frame_count (0..=64)
//! 84      var   panic_msg [u8; panic_msg_len]
//! 84+m    var   frames [u64; frame_count]
//! last    4     crc32 (CRC-32/ISO-HDLC, over all preceding bytes)
//! ```
//!
//! # no_std 合规
//!
//! 仅使用 `core::*` / `alloc::*`。

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

use crate::build_id::BuildId;

/// 转储魔数 `b"ENCD"`（EnerOS Crash Dump）。
pub const MAGIC: [u8; 4] = *b"ENCD";

/// 当前格式版本。
pub const VERSION: u16 = 1;

/// 头部固定长度（不含 panic_msg、frames、crc32）。
pub const HEADER_LEN: usize = 84;

/// 最大 panic 消息长度。
pub const MAX_PANIC_MSG_LEN: usize = 512;

/// 最大帧数。
pub const MAX_FRAMES: usize = 64;


/// 架构标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Arch {
    Unknown = 0,
    Aarch64 = 1,
}

impl Arch {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Aarch64,
            _ => Self::Unknown,
        }
    }
}

/// 单个回溯帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrashFrame {
    /// 该帧的程序计数器（PC）地址。
    pub pc: u64,
}

/// 崩溃转储。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashDump {
    /// 崩溃二进制的 build-id（精确匹配用）。
    pub build_id: BuildId,
    /// 崩溃时间戳（纳秒，0 = 未知）。
    pub crash_ts_ns: u64,
    /// 崩溃线程 ID。
    pub thread_id: u64,
    /// 崩溃时的 PC。
    pub pc: u64,
    /// 崩溃时的 SP。
    pub sp: u64,
    /// 崩溃时的 LR。
    pub lr: u64,
    /// 架构。
    pub arch: Arch,
    /// Panic 消息。
    pub panic_msg: String,
    /// 回溯帧（PC 值）。
    pub frames: Vec<CrashFrame>,
}

/// 编解码错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrashDumpError {
    /// 数据过短。
    TooShort,
    /// 魔数不匹配。
    BadMagic,
    /// 版本不支持。
    UnsupportedVersion,
    /// build_id 长度非法。
    BadBuildIdLen,
    /// panic_msg 长度非法。
    BadPanicMsgLen,
    /// 帧数非法。
    BadFrameCount,
    /// CRC32 校验失败。
    CrcMismatch,
    /// 数据截断。
    Truncated,
}

impl CrashDump {
    /// 创建新的空崩溃转储。
    pub fn new(build_id: BuildId) -> Self {
        Self {
            build_id,
            crash_ts_ns: 0,
            thread_id: 0,
            pc: 0,
            sp: 0,
            lr: 0,
            arch: Arch::Aarch64,
            panic_msg: String::new(),
            frames: Vec::new(),
        }
    }
}

/// 编码崩溃转储为二进制（含 CRC32 尾部）。
pub fn encode(dump: &CrashDump) -> Result<Vec<u8>, CrashDumpError> {
    let panic_bytes = dump.panic_msg.as_bytes();
    if panic_bytes.len() > MAX_PANIC_MSG_LEN {
        return Err(CrashDumpError::BadPanicMsgLen);
    }
    if dump.frames.len() > MAX_FRAMES {
        return Err(CrashDumpError::BadFrameCount);
    }
    let bid = dump.build_id.as_bytes();
    if bid.len() > 32 {
        return Err(CrashDumpError::BadBuildIdLen);
    }

    let total = HEADER_LEN + panic_bytes.len() + dump.frames.len() * 8 + 4;
    let mut buf = Vec::with_capacity(total);

    // 头部
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.push(dump.arch as u8);
    buf.push(bid.len() as u8);
    // build_id zero-padded to 32
    let mut bid_buf = [0u8; 32];
    bid_buf[..bid.len()].copy_from_slice(bid);
    buf.extend_from_slice(&bid_buf);
    buf.extend_from_slice(&dump.crash_ts_ns.to_le_bytes());
    buf.extend_from_slice(&dump.thread_id.to_le_bytes());
    buf.extend_from_slice(&dump.pc.to_le_bytes());
    buf.extend_from_slice(&dump.sp.to_le_bytes());
    buf.extend_from_slice(&dump.lr.to_le_bytes());
    buf.extend_from_slice(&(panic_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(&(dump.frames.len() as u16).to_le_bytes());

    // payload
    buf.extend_from_slice(panic_bytes);
    for frame in &dump.frames {
        buf.extend_from_slice(&frame.pc.to_le_bytes());
    }

    // CRC32
    let crc = crc32(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());

    Ok(buf)
}

/// 从二进制解码崩溃转储。
pub fn decode(data: &[u8]) -> Result<CrashDump, CrashDumpError> {
    if data.len() < HEADER_LEN + 4 {
        return Err(CrashDumpError::TooShort);
    }
    let payload_len = data.len() - 4;
    let payload = &data[..payload_len];

    // CRC 校验
    let stored_crc = u32::from_le_bytes([
        data[payload_len],
        data[payload_len + 1],
        data[payload_len + 2],
        data[payload_len + 3],
    ]);
    let computed_crc = crc32(payload);
    if stored_crc != computed_crc {
        return Err(CrashDumpError::CrcMismatch);
    }

    if payload[..4] != MAGIC {
        return Err(CrashDumpError::BadMagic);
    }
    let version = u16::from_le_bytes([payload[4], payload[5]]);
    if version != VERSION {
        return Err(CrashDumpError::UnsupportedVersion);
    }
    let arch = Arch::from_u8(payload[6]);
    let bid_len = payload[7] as usize;
    if bid_len > 32 {
        return Err(CrashDumpError::BadBuildIdLen);
    }
    let bid = BuildId::from_bytes(&payload[8..8 + bid_len]).ok_or(CrashDumpError::BadBuildIdLen)?;

    let mut off = 40;
    let crash_ts_ns = rd_u64(payload, off)?;
    off += 8;
    let thread_id = rd_u64(payload, off)?;
    off += 8;
    let pc = rd_u64(payload, off)?;
    off += 8;
    let sp = rd_u64(payload, off)?;
    off += 8;
    let lr = rd_u64(payload, off)?;
    off += 8;
    let panic_msg_len = u16::from_le_bytes([payload[off], payload[off + 1]]) as usize;
    off += 2;
    let frame_count = u16::from_le_bytes([payload[off], payload[off + 1]]) as usize;
    off += 2;

    if panic_msg_len > MAX_PANIC_MSG_LEN {
        return Err(CrashDumpError::BadPanicMsgLen);
    }
    if frame_count > MAX_FRAMES {
        return Err(CrashDumpError::BadFrameCount);
    }

    // off == HEADER_LEN (84)
    if off + panic_msg_len + frame_count * 8 != payload_len {
        return Err(CrashDumpError::Truncated);
    }

    let panic_msg = core::str::from_utf8(&payload[off..off + panic_msg_len])
        .map_err(|_| CrashDumpError::Truncated)?
        .to_owned();
    off += panic_msg_len;

    let mut frames = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        let fpc = rd_u64(payload, off)?;
        off += 8;
        frames.push(CrashFrame { pc: fpc });
    }

    Ok(CrashDump {
        build_id: bid,
        crash_ts_ns,
        thread_id,
        pc,
        sp,
        lr,
        arch,
        panic_msg,
        frames,
    })
}

fn rd_u64(buf: &[u8], off: usize) -> Result<u64, CrashDumpError> {
    if off + 8 > buf.len() {
        return Err(CrashDumpError::Truncated);
    }
    Ok(u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ]))
}

/// CRC-32/ISO-HDLC（与 zlib/gzip 一致）。
pub fn crc32(data: &[u8]) -> u32 {
    // HYG-001：CRC32 唯一实现归一到 abi/ipc-wire。
    eneros_ipc_wire::crc32(data)
}
