//! 真实命令下发 sink（21-B A1：生产路径零 mock）。
//!
//! 将 `DispatchCommand` 以 JSON Lines 形式写入指定文件，并维护写入计数
//! 供 Fatal 阻断验收（P0-17 语义：Fatal 违规下 sink 必须零写入）。

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use eneros_control_gateway::gateway::AuthorizedCommand;
use eneros_dual_brain::chain_sink::{DeviceExecError, DeviceExecutor};
use eneros_dual_brain::sink::{CommandSink, DispatchCommand};
use eneros_dual_brain::DualBrainError;

/// 文件命令 sink：每条命令序列化为一行 JSON，追加写入 `path`。
pub struct FileCommandSink {
    path: PathBuf,
    file: File,
    written: u64,
}

impl FileCommandSink {
    /// 打开（或创建）sink 文件。
    ///
    /// 失败返回 `RunnerError::Io`；sink 打开失败即装配失败（fail-closed，
    /// 不允许"命令无处可去"的降级运行）。
    pub fn new(path: &str) -> Result<Self, RunnerIoError> {
        let file = File::options()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| RunnerIoError {
                path: path.to_owned(),
                source: e,
            })?;
        Ok(Self {
            path: PathBuf::from(path),
            file,
            written: 0,
        })
    }

    /// sink 文件路径（证据输出用）.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// sink 文件 I/O 错误（路径 + 底层错误）.
#[derive(Debug)]
pub struct RunnerIoError {
    pub path: String,
    pub source: std::io::Error,
}

impl CommandSink for FileCommandSink {
    fn write(&mut self, cmd: DispatchCommand) -> Result<(), DualBrainError> {
        let line = serde_json::json!({
            "schema": "eneros.command-sink.v1",
            "target_device": cmd.target_device,
            "power_kw": cmd.power_kw,
            "ttl_secs": cmd.ttl_secs,
            "timestamp": cmd.timestamp,
        });
        let mut bytes = serde_json::to_vec(&line).map_err(|e| {
            DualBrainError::DispatchError(format!("sink json serialize failed: {e}"))
        })?;
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .map_err(|e| DualBrainError::DispatchError(format!("sink write failed: {e}")))?;
        self.file
            .flush()
            .map_err(|e| DualBrainError::DispatchError(format!("sink flush failed: {e}")))?;
        self.written += 1;
        Ok(())
    }

    fn write_count(&self) -> usize {
        self.written as usize
    }
}

/// 文件执行器：授权命令 → JSON Lines 落盘（复用 FileCommandSink 通道）。
pub struct FileExecutor {
    inner: FileCommandSink,
}

impl FileExecutor {
    /// 打开文件执行器（sink 文件必填——fail-closed）。
    pub fn new(path: &str) -> Result<Self, RunnerIoError> {
        Ok(Self {
            inner: FileCommandSink::new(path)?,
        })
    }

    /// 文件路径（证据输出用）。
    pub fn path(&self) -> &std::path::Path {
        self.inner.path()
    }
}

impl DeviceExecutor for FileExecutor {
    fn write(
        &mut self,
        cmd: &AuthorizedCommand,
        envelope: &[u8],
        power_kw: f64,
    ) -> Result<(), DeviceExecError> {
        // 授权命令 → 文件总线（演示/测试装配；§6.4 FileCommandSink 保留）。
        // 信封字节（hex）随行落盘，供证据追溯。
        let envelope_hex = envelope
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let line = serde_json::json!({
            "schema": "eneros.executor-file.v1",
            "envelope_hex": envelope_hex,
            "device_id": cmd.device_id(),
            "point_id": cmd.point_id(),
            "power_kw": power_kw,
        });
        let mut bytes = serde_json::to_vec(&line)
            .map_err(|e| DeviceExecError::Comm(format!("executor json serialize failed: {e}")))?;
        bytes.push(b'\n');
        self.inner
            .file
            .write_all(&bytes)
            .map_err(|e| DeviceExecError::Comm(format!("executor write failed: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_writes_json_lines_and_counts() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-ai-runner-sink-write-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("commands.jsonl");
        let path_str = path.to_str().unwrap().to_owned();

        let mut sink = FileCommandSink::new(&path_str).expect("sink open");
        assert_eq!(sink.write_count(), 0);

        sink.write(DispatchCommand {
            target_device: "pcs".into(),
            power_kw: 12.5,
            ttl_secs: 300,
            timestamp: 42,
        })
        .expect("write");

        // write 后计数 +1，文件含一行 JSON。
        assert_eq!(sink.write_count(), 1);
        let content = std::fs::read_to_string(&path).expect("read back");
        let line = content.lines().next().expect("one line");
        let parsed: serde_json::Value = serde_json::from_str(line).expect("valid json");
        assert_eq!(parsed["target_device"], "pcs");
        assert_eq!(parsed["power_kw"], 12.5);
        assert_eq!(parsed["timestamp"], 42);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sink_append_preserves_prior_lines() {
        let dir = std::env::temp_dir().join(format!(
            "eneros-ai-runner-sink-append-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("commands-append.jsonl");
        let path_str = path.to_str().unwrap().to_owned();

        {
            let mut sink = FileCommandSink::new(&path_str).expect("sink open");
            sink.write(DispatchCommand {
                target_device: "pcs".into(),
                power_kw: 1.0,
                ttl_secs: 1,
                timestamp: 1,
            })
            .expect("write");
        }
        let mut sink = FileCommandSink::new(&path_str).expect("reopen");
        sink.write(DispatchCommand {
            target_device: "pcs".into(),
            power_kw: 2.0,
            ttl_secs: 2,
            timestamp: 2,
        })
        .expect("write");

        let content = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(content.lines().count(), 2, "append 模式必须保留旧行");

        std::fs::remove_dir_all(&dir).ok();
    }
}

// ============================================================================
// 22 §2.W2.2：FileExecutor——ChainCommandSink 的设备执行器适配器。
// ControlChain 授权后的命令经文件总线落盘（真实设备通道；AI 不直接写设备）。
// ============================================================================
