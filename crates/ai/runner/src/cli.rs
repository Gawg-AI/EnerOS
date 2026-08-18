//! 32 AI-07：CLI 参数解析（fail-closed 密钥注入）。
//!
//! 从 `main.rs` 抽出为 lib 模块——判据"无 key 文件 → 退出码非 0"
//! 必须 host 可测（蓝图 32 §7 验证）。`main.rs` 消费本模块，测试
//! 直接注入 argv。

use crate::scenario::Scenario;

/// CLI 参数（全部显式注入；模型路径/sink 目标禁止硬编码——21-B §2.A1）.
#[derive(Debug)]
pub struct CliArgs {
    /// llama.cpp GGUF 模型路径（必填）.
    pub model: String,
    /// 命令 sink 文件路径（必填，JSON Lines）.
    pub sink: String,
    /// 场景名.
    pub scenario: Scenario,
    /// 决策时刻（默认系统真实时钟）.
    pub now_ms: u64,
    /// 覆盖 `ScheduleConfig.soc_min`.
    pub soc_min: Option<f64>,
    /// 32 AI-07：链密钥（--chain-key-file 读取；缺省拒绝启动）。
    pub chain_key: Vec<u8>,
    /// 蓝图 40 WP-40.5：显式启用 demo 可信反馈（仅 DEMO_ONLY 证据；
    /// 缺省 false = untrusted → 授权链拒绝，demo 诚实失败）。
    pub demo_trusted_feedback: bool,
}

/// 解析 CLI 参数（`argv` 不含程序名；`main.rs` 传 `env::args().skip(1)`）。
///
/// fail-closed（32 AI-07）：`--chain-key-file` 缺省/读取失败/密钥 <16B
/// 一律 Err（调用方据此非零退出）。
pub fn parse_cli_args<I, S>(mut it: I) -> Result<CliArgs, String>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let mut model = None;
    let mut sink = None;
    let mut chain_key: Option<Vec<u8>> = None;
    let mut scenario = Scenario::Normal;
    let mut now_ms = None;
    let mut soc_min = None;
    let mut demo_trusted_feedback = false;

    while let Some(flag_ref) = it.next() {
        let flag = flag_ref.as_ref().to_string();
        if flag == "--demo-trusted-feedback" {
            demo_trusted_feedback = true;
            continue;
        }
        let value_ref = it
            .next()
            .ok_or_else(|| format!("{flag} requires a value"))?;
        let value = value_ref.as_ref().to_string();
        match flag.as_str() {
            "--model" => model = Some(value),
            "--sink" => sink = Some(value),
            // 32 AI-07：链密钥文件（fail-closed 缺省拒绝）。
            "--chain-key-file" => {
                let raw = std::fs::read(&value)
                    .map_err(|e| format!("--chain-key-file read failed: {e}"))?;
                let key: Vec<u8> = raw
                    .into_iter()
                    .filter(|b| *b != b'\n' && *b != b'\r')
                    .collect();
                if key.len() < 16 {
                    return Err("chain key must be >=16 bytes".into());
                }
                chain_key = Some(key);
            }
            "--scenario" => scenario = Scenario::parse(&value)?,
            "--now-ms" => {
                now_ms = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| "--now-ms must be an unsigned integer")?,
                )
            }
            "--soc-min" => {
                soc_min = Some(
                    value
                        .parse::<f64>()
                        .map_err(|_| "--soc-min must be a float")?,
                )
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }

    Ok(CliArgs {
        chain_key: chain_key
            .ok_or_else(|| "--chain-key-file is mandatory (fail-closed; 32 AI-07)".to_owned())?,
        model: model.ok_or_else(|| "--model is mandatory".to_owned())?,
        sink: sink.ok_or_else(|| "--sink is mandatory".to_owned())?,
        scenario,
        now_ms: now_ms.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        }),
        soc_min,
        demo_trusted_feedback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落一个临时密钥文件（测试用固定密钥，≥16B）。
    fn write_key(dir: &std::path::Path, name: &str, content: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, content).expect("write key");
        p.to_string_lossy().into_owned()
    }

    /// 32 AI-07 判据：无 --chain-key-file → Err（main 据此 exit 2 非零）。
    #[test]
    fn no_chain_key_file_rejected_fail_closed() {
        let r = parse_cli_args(
            ["--model", "m.gguf", "--sink", "s.jsonl"]
                .into_iter()
                .map(str::to_string),
        );
        let err = r.expect_err("缺密钥必须拒绝");
        assert!(err.contains("--chain-key-file is mandatory"), "{err}");
    }

    /// 32 AI-07 判据：密钥文件不存在（路径无效）→ Err（非零退出）。
    #[test]
    fn missing_chain_key_file_rejected() {
        let r = parse_cli_args(
            [
                "--chain-key-file",
                "/nonexistent/eneros-test-key-nope",
                "--model",
                "m.gguf",
                "--sink",
                "s.jsonl",
            ]
            .into_iter()
            .map(str::to_string),
        );
        let err = r.expect_err("文件缺失必须拒绝");
        assert!(err.contains("read failed"), "{err}");
    }

    /// 32 AI-07 判据：密钥 <16B → Err（长度校验 fail-closed）。
    #[test]
    fn short_chain_key_rejected() {
        let dir = std::env::temp_dir();
        let key_path = write_key(&dir, "eneros-ai07-short.key", b"short-key");
        let r = parse_cli_args(
            [
                "--chain-key-file",
                &key_path,
                "--model",
                "m.gguf",
                "--sink",
                "s.jsonl",
            ]
            .into_iter()
            .map(str::to_string),
        );
        let err = r.expect_err("短密钥必须拒绝");
        assert!(err.contains(">=16 bytes"), "{err}");
        let _ = std::fs::remove_file(&key_path);
    }

    /// 正向：合法密钥（换行过滤后 ≥16B）→ 解析成功。
    #[test]
    fn valid_chain_key_accepted() {
        let dir = std::env::temp_dir();
        let key_path = write_key(
            &dir,
            "eneros-ai07-valid.key",
            b"0123456789abcdef\n", // 16B + 换行（读取时过滤）
        );
        let args = parse_cli_args(
            [
                "--chain-key-file",
                &key_path,
                "--model",
                "m.gguf",
                "--sink",
                "s.jsonl",
                "--now-ms",
                "42",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .expect("合法参数必须通过");
        assert_eq!(args.chain_key.len(), 16);
        assert_eq!(args.model, "m.gguf");
        assert_eq!(args.now_ms, 42);
        let _ = std::fs::remove_file(&key_path);
    }
}
