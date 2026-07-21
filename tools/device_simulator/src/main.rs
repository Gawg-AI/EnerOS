//! EnerOS Device Simulator v0.51.2 (D11: host-side std tool)
//!
//! 模拟现场设备（Modbus RTU/TCP、IEC 104、CAN），用于工厂量产与协议开发
//! 阶段的主机侧调试。本程序为 **std** 程序（D11），不受 no_std 约束。
//!
//! v0.51.2 骨架工具链：`sim` 模块定义的公共 API 中部分类型/字段在
//! stub `main` 中尚未被引用，按骨架阶段约定允许 dead_code。

#![allow(dead_code)]

mod sim;

use std::env;
use std::process::ExitCode;

fn print_help() {
    println!("EnerOS Device Simulator v0.51.2");
    println!("===============================");
    println!();
    println!("USAGE:");
    println!("    eneros-device-simulator [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    --help                      Print this help message");
    println!("    --config <file>             Load simulator configuration from file");
    println!("    --protocol <PROTOCOL>       Simulated protocol:");
    println!("                                    modbus-rtu | modbus-tcp | iec104 | can");
    println!();
    println!("EXAMPLES:");
    println!("    eneros-device-simulator --protocol modbus-rtu");
    println!("    eneros-device-simulator --config sim.toml --protocol iec104");
}

fn main() -> ExitCode {
    println!("EnerOS Device Simulator v0.51.2");
    println!("===============================");

    let args: Vec<String> = env::args().collect();
    let mut protocol: Option<String> = None;
    let mut config_file: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--config" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --config requires a file argument");
                    return ExitCode::FAILURE;
                }
                config_file = Some(args[i].clone());
            }
            "--protocol" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --protocol requires a protocol argument");
                    return ExitCode::FAILURE;
                }
                let p = args[i].clone();
                if !matches!(p.as_str(), "modbus-rtu" | "modbus-tcp" | "iec104" | "can") {
                    eprintln!("error: unsupported protocol '{}'", p);
                    eprintln!("       supported: modbus-rtu | modbus-tcp | iec104 | can");
                    return ExitCode::FAILURE;
                }
                protocol = Some(p);
            }
            other => {
                eprintln!("error: unknown argument '{}'", other);
                eprintln!("       run with --help for usage");
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    let protocol = protocol.unwrap_or_else(|| "modbus-rtu".to_string());
    println!("[sim] protocol   : {}", protocol);
    println!("[sim] config     : {}", config_file.as_deref().unwrap_or("<none>"));

    let config = sim::SimConfig {
        protocol: protocol.clone(),
        port: 502,
        slave_addr: 1,
        baud_rate: 9600,
        ip: "127.0.0.1".to_string(),
        point_count: 100,
    };

    let mut handle = sim::SimHandle::new(config);
    if let Err(e) = handle.start() {
        eprintln!("[sim] start failed: {}", e);
        return ExitCode::FAILURE;
    }

    println!("[sim] simulator running (stub)");

    // Stub: generate a dummy response for a sample request frame.
    let request: &[u8] = &[0x01, 0x03, 0x00, 0x00, 0x00, 0x0A];
    let response = handle.generate_response(request);
    println!(
        "[sim] sample response ({} bytes): {:02X?}",
        response.len(),
        response
    );

    if let Err(e) = handle.stop() {
        eprintln!("[sim] stop failed: {}", e);
        return ExitCode::FAILURE;
    }

    println!("[sim] simulator stopped");
    ExitCode::SUCCESS
}
