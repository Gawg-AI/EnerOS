//! EnerOS Batch Configuration Tool v0.51.2 (D11: host-side std tool)
//!
//! 将配置模板批量下发到一组设备，并对每台设备运行工厂测试套件。
//! 本程序为 **std** 程序（D11），不受 no_std 约束。
//!
//! v0.51.2 骨架工具链：`runner` 模块定义的公共 API 中部分类型/字段在
//! stub `main` 中尚未被引用，按骨架阶段约定允许 dead_code。

#![allow(dead_code)]

mod runner;

use std::env;
use std::process::ExitCode;

use runner::FactoryTestRunner;

fn print_help() {
    println!("EnerOS Batch Configuration Tool v0.51.2");
    println!("========================================");
    println!();
    println!("USAGE:");
    println!("    eneros-batch-config [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    --help              Print this help message");
    println!("    --template <file>   Configuration template file to push to devices");
    println!("    --devices <file>    Device list file (one address per line)");
    println!("    --dry-run           Validate inputs without touching devices");
    println!();
    println!("EXAMPLES:");
    println!("    eneros-batch-config --template tpl.toml --devices devices.txt");
    println!("    eneros-batch-config --template tpl.toml --devices devices.txt --dry-run");
}

fn main() -> ExitCode {
    println!("EnerOS Batch Configuration Tool v0.51.2");
    println!("========================================");

    let args: Vec<String> = env::args().collect();
    let mut template: Option<String> = None;
    let mut devices: Option<String> = None;
    let mut dry_run = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--template" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --template requires a file argument");
                    return ExitCode::FAILURE;
                }
                template = Some(args[i].clone());
            }
            "--devices" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --devices requires a file argument");
                    return ExitCode::FAILURE;
                }
                devices = Some(args[i].clone());
            }
            "--dry-run" => {
                dry_run = true;
            }
            other => {
                eprintln!("error: unknown argument '{}'", other);
                eprintln!("       run with --help for usage");
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    println!(
        "[batch] template : {}",
        template.as_deref().unwrap_or("<none>")
    );
    println!(
        "[batch] devices  : {}",
        devices.as_deref().unwrap_or("<none>")
    );
    println!("[batch] dry-run  : {}", dry_run);

    // 构造一个示例工厂测试套件并通过默认 runner 运行。
    let suite = runner::TestSuite {
        name: "smoke".to_string(),
        items: vec![
            runner::TestItem {
                name: "ping".to_string(),
                category: runner::TestCategory::Communication,
                passed: false,
                failure_reason: None,
                duration_ms: 0,
            },
            runner::TestItem {
                name: "read_point_table".to_string(),
                category: runner::TestCategory::Functional,
                passed: false,
                failure_reason: None,
                duration_ms: 0,
            },
        ],
    };

    let mut r = runner::DefaultTestRunner::new();
    let report = r.run_suite(&suite);
    println!();
    println!("{}", report.summary());

    ExitCode::SUCCESS
}
