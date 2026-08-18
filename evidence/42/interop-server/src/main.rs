//! 42（WP-42.7/42.8）：正式 L1 互操作对端测试用 EnerOS Modbus TCP
//! slave 宿主进程。独立 raw peer（无 EnerOS 帧代码）连接本进程完成
//! 真实事务/负例/抓包。仅测试证据用，不进产品闭包。

use eneros_modbus_tcp_server::ModbusTcpServer;
use std::env;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:15020".to_owned());
    let mut server = match ModbusTcpServer::new(&addr, 16) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("interop-server: bind {addr} failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    // 种子值：0x1111/0x2222/0x3333 —— raw peer 断言回读一致。
    server.seed_register(0, 0x1111);
    server.seed_register(1, 0x2222);
    server.seed_register(2, 0x3333);
    eprintln!("interop-server: ready on {addr}");
    // 一次性 handler 循环直到被外部结束（每次 serve_one 阻塞等连接）。
    let deadline = Duration::from_secs(120);
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        match server.serve_one() {
            Ok(_) => {}
            Err(e) => eprintln!("interop-server: connection error: {e}"),
        }
    }
    eprintln!("interop-server: deadline reached, exiting");
    ExitCode::SUCCESS
}