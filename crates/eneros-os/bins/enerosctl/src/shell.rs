//! 交互式 shell 模式（v0.28.0 — Task 13）
//!
//! 提供 REPL 交互式 shell，支持命令历史、Tab 补全。
//! 通过 `enerosctl shell` 进入，输入 `exit` 退出。

use clap::Parser;
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper, Result};
use rustyline::history::{DefaultHistory, History};
use std::path::PathBuf;

/// 所有可补全的顶层命令名（含 shell 内建命令）
const SUBCOMMANDS: &[&str] = &[
    "agent",
    "eventbus",
    "system",
    "network",
    "log",
    "device",
    "audit",
    "time",
    "update",
    "protocol",
    "security",
    "ha",
    "plugin",
    "simulator",
    "shell",
    "completions",
    "config",
    "service",
    "doctor",
    "help",
    "exit",
    "quit",
    "clear",
    "history",
];

/// Tab 补全 Helper
struct ShellHelper;

impl Completer for ShellHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<String>)> {
        // 只对第一个 token（顶层子命令）做补全
        let prefix = &line[..pos];
        // 取当前行中第一个空白之前的部分作为补全前缀
        let token_end = prefix.find(char::is_whitespace).unwrap_or(pos);
        let token = &prefix[..token_end];

        let mut candidates: Vec<String> = SUBCOMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(token))
            .map(|cmd| (*cmd).to_string())
            .collect();
        candidates.sort();
        Ok((0, candidates))
    }
}

impl Hinter for ShellHelper {
    type Hint = String;
}

impl Validator for ShellHelper {}
impl Highlighter for ShellHelper {}
impl Helper for ShellHelper {}

/// 交互式 shell
pub struct InteractiveShell {
    editor: Editor<ShellHelper, DefaultHistory>,
    history_path: PathBuf,
    socket: String,
}

impl InteractiveShell {
    /// 创建新的交互式 shell
    pub fn new(socket: &str) -> Result<Self> {
        let mut editor: Editor<ShellHelper, DefaultHistory> = Editor::new()?;
        editor.set_helper(Some(ShellHelper));

        let history_path = dirs::home_dir()
            .map(|h| h.join(".eneros/history.txt"))
            .unwrap_or_else(|| PathBuf::from(".eneros_history.txt"));

        // 尝试加载历史（文件不存在时忽略错误）
        let _ = editor.load_history(&history_path);

        Ok(Self {
            editor,
            history_path,
            socket: socket.to_string(),
        })
    }

    /// 返回历史文件路径（供测试使用）
    pub fn history_path(&self) -> &std::path::Path {
        &self.history_path
    }

    /// 返回帮助文本（供测试使用）
    pub fn help_text(&self) -> String {
        let mut buf = String::new();
        buf.push_str("EnerOS 交互式 shell 命令列表:\n");
        buf.push_str("\n  内建命令:\n");
        buf.push_str("    help    — 显示此帮助\n");
        buf.push_str("    history — 显示命令历史\n");
        buf.push_str("    clear   — 清屏\n");
        buf.push_str("    exit    — 退出 shell（或 quit / Ctrl+D）\n");
        buf.push_str("\n  管理命令:\n");
        buf.push_str("    agent       — Agent 进程管理\n");
        buf.push_str("    eventbus    — EventBus 事件总线管理\n");
        buf.push_str("    system      — 系统信息\n");
        buf.push_str("    network     — 网络配置管理\n");
        buf.push_str("    log         — 日志管理\n");
        buf.push_str("    device      — 设备管理\n");
        buf.push_str("    audit       — 审计日志管理\n");
        buf.push_str("    time        — 时间同步管理\n");
        buf.push_str("    update      — OTA 更新管理\n");
        buf.push_str("    protocol    — 协议适配器管理\n");
        buf.push_str("    security    — 安全管理\n");
        buf.push_str("    ha          — 高可用管理\n");
        buf.push_str("    plugin      — 插件管理\n");
        buf.push_str("    simulator   — 模拟器管理\n");
        buf.push_str("    shell       — 启动交互式 shell\n");
        buf.push_str("    completions — 生成 shell 补全脚本\n");
        buf.push_str("    config      — 配置管理\n");
        buf.push_str("    service     — 服务管理\n");
        buf.push_str("    doctor      — 系统诊断\n");
        buf
    }

    /// 判断是否为退出命令
    pub fn is_exit(line: &str) -> bool {
        matches!(line.trim(), "exit" | "quit")
    }

    /// 判断是否为清屏命令
    pub fn is_clear(line: &str) -> bool {
        line.trim() == "clear"
    }

    /// 判断是否为历史命令
    pub fn is_history(line: &str) -> bool {
        line.trim() == "history"
    }

    /// 判断是否为帮助命令
    pub fn is_help(line: &str) -> bool {
        line.trim() == "help"
    }

    /// 显示帮助
    fn show_help(&self) {
        print!("{}", self.help_text());
    }

    /// 显示历史
    fn show_history(&self) {
        // 遍历 rustyline 内部历史并逐条打印
        let history = self.editor.history();
        if history.is_empty() {
            println!("（无命令历史）");
        } else {
            for (i, entry) in history.iter().enumerate() {
                println!("{:>4}  {}", i + 1, entry);
            }
            println!(
                "（共 {} 条历史记录，保存至 {}）",
                history.len(),
                self.history_path().display()
            );
        }
    }

    /// 运行 REPL 循环
    pub async fn run(&mut self) -> Result<()> {
        println!("EnerOS 交互式 shell（输入 help 查看帮助，exit 退出）");
        loop {
            match self.editor.readline("eneros> ") {
                Ok(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    // 历史记录为非关键功能，失败不应影响 shell 运行
                    let _ = self.editor.add_history_entry(line);

                    // 内建命令
                    if Self::is_exit(line) {
                        break;
                    }
                    if Self::is_clear(line) {
                        print!("\x1b[2J\x1b[H");
                        continue;
                    }
                    if Self::is_history(line) {
                        self.show_history();
                        continue;
                    }
                    if Self::is_help(line) {
                        self.show_help();
                        continue;
                    }

                    // 执行管理命令
                    self.execute_command(line).await;
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("exit");
                    break;
                }
                Err(err) => {
                    eprintln!("错误: {:?}", err);
                    break;
                }
            }
        }
        // 保存历史
        let _ = self.editor.save_history(&self.history_path);
        Ok(())
    }

    /// 解析并执行命令（调用 clap 解析 + dispatch_command）
    async fn execute_command(&mut self, line: &str) {
        // 将输入行拆分为参数，前置程序名 "enerosctl" 供 clap 解析
        let args: Vec<&str> = std::iter::once("enerosctl")
            .chain(line.split_whitespace())
            .collect();

        match crate::Cli::try_parse_from(args) {
            Ok(cli) => {
                // 拦截 Shell 子命令，避免递归
                if matches!(cli.command, crate::Commands::Shell) {
                    println!("已在交互式 shell 中，无需重复启动。");
                    return;
                }
                // 分发到实际命令处理逻辑
                // Box::pin 避免 async 递归导致的无限大小 future
                let fut = Box::pin(crate::commands::dispatch_command(cli.command, &self.socket));
                if let Err(e) = fut.await {
                    eprintln!("错误: {}", e);
                }
            }
            Err(e) => {
                // clap 解析失败，打印错误信息但不退出进程
                eprintln!("{}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试创建 InteractiveShell 实例
    #[test]
    fn test_shell_new() {
        let shell = InteractiveShell::new("/var/run/eneros/control.sock");
        assert!(shell.is_ok(), "InteractiveShell 创建应成功");
    }

    /// 测试历史文件路径
    #[test]
    fn test_shell_history_path() {
        let shell = InteractiveShell::new("/var/run/eneros/control.sock")
            .expect("创建 shell 失败");
        let path = shell.history_path();
        let path_str = path.to_string_lossy();
        // 路径应包含 .eneros 或 fallback 到 .eneros_history.txt
        assert!(
            path_str.contains(".eneros") || path_str.contains("eneros_history"),
            "历史路径应包含 .eneros，实际: {}",
            path_str
        );
    }

    /// 测试帮助输出包含所有子命令
    #[test]
    fn test_shell_help_output() {
        let shell = InteractiveShell::new("/var/run/eneros/control.sock")
            .expect("创建 shell 失败");
        let help = shell.help_text();
        // 验证所有顶层子命令都出现在帮助文本中
        for cmd in &[
            "agent",
            "eventbus",
            "system",
            "network",
            "log",
            "device",
            "audit",
            "time",
            "update",
            "protocol",
            "security",
            "ha",
            "plugin",
            "simulator",
            "shell",
            "completions",
            "config",
            "service",
            "doctor",
        ] {
            assert!(
                help.contains(cmd),
                "帮助文本应包含 '{}'，实际: {}",
                cmd,
                help
            );
        }
    }

    /// 测试 exit 命令识别
    #[test]
    fn test_shell_exit_command() {
        assert!(InteractiveShell::is_exit("exit"));
        assert!(InteractiveShell::is_exit("quit"));
        assert!(InteractiveShell::is_exit("  exit  "));
        assert!(!InteractiveShell::is_exit("agent"));
        assert!(!InteractiveShell::is_exit("shell"));
    }

    /// 测试 clear 命令识别
    #[test]
    fn test_shell_clear_command() {
        assert!(InteractiveShell::is_clear("clear"));
        assert!(InteractiveShell::is_clear("  clear  "));
        assert!(!InteractiveShell::is_clear("exit"));
        assert!(!InteractiveShell::is_clear("agent"));
    }

    /// 测试 history 命令识别
    #[test]
    fn test_shell_history_command() {
        assert!(InteractiveShell::is_history("history"));
        assert!(InteractiveShell::is_history("  history  "));
        assert!(!InteractiveShell::is_history("exit"));
        assert!(!InteractiveShell::is_history("help"));
    }

    /// 测试 bash 补全脚本生成
    #[test]
    fn test_completions_bash() {
        let script = crate::commands::generate_completions(clap_complete::Shell::Bash);
        assert!(
            script.contains("enerosctl"),
            "bash 补全脚本应包含 'enerosctl'，实际: {}",
            &script[..script.len().min(200)]
        );
    }

    /// 测试 zsh 补全脚本生成
    #[test]
    fn test_completions_zsh() {
        let script = crate::commands::generate_completions(clap_complete::Shell::Zsh);
        assert!(
            script.contains("enerosctl"),
            "zsh 补全脚本应包含 'enerosctl'，实际: {}",
            &script[..script.len().min(200)]
        );
    }
}
