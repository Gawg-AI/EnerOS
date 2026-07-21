//! EnerOS Protocol Analyzer v0.51.2 (D11: host-side std tool)
//!
//! 抓取并分析协议流量（Modbus / IEC 104 / CAN），用于工厂量产与现场
//! 运维阶段的总线诊断。本程序为 **std** 程序（D11），不受 no_std 约束。
//!
//! v0.51.2 骨架工具链：`capture` 模块定义的公共 API 中部分类型/字段在
//! stub `main` 中尚未被引用，按骨架阶段约定允许 dead_code。

#![allow(dead_code)]

mod capture;

use std::env;
use std::process::ExitCode;

fn print_help() {
    println!("EnerOS Protocol Analyzer v0.51.2");
    println!("================================");
    println!();
    println!("USAGE:");
    println!("    eneros-protocol-analyzer [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    --help                Print this help message");
    println!("    --interface <if>      Network interface to capture on (e.g. eth0)");
    println!("    --port <port>         TCP/UDP port to filter (0 = all)");
    println!("    --protocol <PROTO>    Protocol to decode: modbus | iec104 | can");
    println!();
    println!("EXAMPLES:");
    println!("    eneros-protocol-analyzer --interface eth0 --protocol modbus --port 502");
}

fn main() -> ExitCode {
    println!("EnerOS Protocol Analyzer v0.51.2");
    println!("================================");

    let args: Vec<String> = env::args().collect();
    let mut interface: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut protocol: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--interface" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --interface requires an argument");
                    return ExitCode::FAILURE;
                }
                interface = Some(args[i].clone());
            }
            "--port" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --port requires an argument");
                    return ExitCode::FAILURE;
                }
                match args[i].parse::<u16>() {
                    Ok(p) => port = Some(p),
                    Err(_) => {
                        eprintln!("error: invalid port number '{}'", args[i]);
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--protocol" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --protocol requires an argument");
                    return ExitCode::FAILURE;
                }
                let p = args[i].clone();
                if !matches!(p.as_str(), "modbus" | "iec104" | "can") {
                    eprintln!("error: unsupported protocol '{}'", p);
                    eprintln!("       supported: modbus | iec104 | can");
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

    let interface = interface.unwrap_or_else(|| "eth0".to_string());
    let port = port.unwrap_or(0);
    let protocol = protocol.unwrap_or_else(|| "modbus".to_string());

    println!("[analyzer] interface : {}", interface);
    println!("[analyzer] port      : {}", port);
    println!("[analyzer] protocol  : {}", protocol);

    let config = capture::CaptureConfig {
        interface: interface.clone(),
        port,
        protocol: protocol.clone(),
        max_packets: 1024,
    };

    let mut cap = capture::PacketCapture::new(config);
    match cap.capture(1000) {
        Ok(n) => println!("[analyzer] captured {} packets", n),
        Err(e) => {
            eprintln!("[analyzer] capture failed: {}", e);
            return ExitCode::FAILURE;
        }
    }

    let stats = cap.analyze();
    println!(
        "[analyzer] stats: total={} rx={} tx={}",
        stats.total_packets, stats.rx_count, stats.tx_count
    );
    for (proto, count) in &stats.protocol_breakdown {
        println!("    {}: {}", proto, count);
    }

    ExitCode::SUCCESS
}
