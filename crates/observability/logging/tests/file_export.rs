#![cfg(feature = "std")]
//! 33 OB-07 端到端导出证据链（host std 环境）。
//!
//! record → flush → JSONL 文件可读回：经 `--features std` 启用
//! `eneros-logging` 的 FileStorage 后端，验证日志从内存缓冲批量落盘后
//! 可由文件读回并经 LogFilter 查询（商用验收的 host 可跑证据链）。
//!
//! 运行：`cargo test -p eneros-logging --features std`
//! （默认 `cargo test --workspace` 下本文件整体 cfg 关闭，no_std 面不受影响）

// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use eneros_logging::{
    FileStorage, LogBuilder, LogFilter, LogLevel, LogStorage, StructuredLogger,
    StructuredLoggerImpl,
};

#[cfg(feature = "std")]
#[test]
fn e2e_record_flush_jsonl_readback() {
    let dir = std::env::temp_dir().join(format!("eneros-log-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("export.jsonl");
    let path_str = path.to_str().unwrap();

    // 1) record：多级日志入内存缓冲
    let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
    logger
        .log(
            LogBuilder::new(LogLevel::Info, "agent.energy", "启动完成", 1000)
                .agent(1)
                .build(),
        )
        .unwrap();
    logger
        .log(
            LogBuilder::new(LogLevel::Error, "agent.pv", "功率越限", 2000)
                .agent(2)
                .field("power_kw", "120.5")
                .build(),
        )
        .unwrap();

    // 2) flush：批量落盘为 JSONL（每条一行）
    let mut store = FileStorage::open(path_str).unwrap();
    let n = logger.flush_to(&mut store).unwrap();
    assert_eq!(n, 2);
    store.flush().unwrap();

    // 3) 读回：磁盘 JSONL 行数与内容逐字段一致
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw.lines().count(), 2, "JSONL 应恰两行");
    assert!(raw.contains("\"msg\":\"启动完成\""), "JSONL 可读（含中文）");

    let read_back = store.read_all().unwrap();
    assert_eq!(read_back.len(), 2);
    assert_eq!(read_back[0].level, LogLevel::Info);
    assert_eq!(read_back[0].message.as_str(), "启动完成");
    assert_eq!(read_back[1].level, LogLevel::Error);
    assert_eq!(read_back[1].fields[0].1.as_str(), "120.5");

    // 4) 重启语义：新实例打开同一文件 → LogFilter 查询命中
    let reopened = FileStorage::open(path_str).unwrap();
    let filter = LogFilter {
        level_min: Some(LogLevel::Error),
        ..Default::default()
    };
    let got = reopened.query(&filter).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].target.as_str(), "agent.pv");

    let _ = std::fs::remove_dir_all(&dir);
}
