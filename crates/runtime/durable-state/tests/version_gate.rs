//! 蓝图 61 §4.4：record 版本门——CRC 完整但 schema 未知的记录必须显式
//! UnsupportedVersion（回滚旧 binary 遇新记录：不误解字节、不格式化）；
//! 撕裂/垃圾槽保持 Corrupt（跳过重建）。

// 测试断言 panic（仓内测试模块惯例：clippy.toml 非测试代码禁 panic）。
#![allow(clippy::disallowed_macros)]

use eneros_durable_state::{decode_record, encode_record, StoreError};

#[test]
fn unknown_schema_with_valid_crc_is_unsupported() {
    // 伪造"未来版本"记录：schema=2 + 合法 CRC（重算 CRC 使其完整）。
    let payload = b"future-state-bytes";
    let mut rec = encode_record(b"TEST", 1, payload);
    rec[4] = 2; // 抬版本（CRC 覆盖 [0..9+len]，需重算）
    let len = payload.len();
    let crc = eneros_ipc_wire::crc32(&rec[..9 + len]);
    rec[9 + len..13 + len].copy_from_slice(&crc.to_le_bytes());

    match decode_record(&rec) {
        Err(StoreError::UnsupportedVersion { schema, supported }) => {
            assert_eq!((schema, supported), (2, 1));
        }
        other => panic!("期望 UnsupportedVersion，got {other:?}"),
    }
}

#[test]
fn garbage_tail_stays_corrupt() {
    // 0xFF 撕裂槽 → Corrupt（不是新版本）。
    let junk = [0xFFu8; 64];
    assert_eq!(decode_record(&junk), Err(StoreError::Corrupt));
}
