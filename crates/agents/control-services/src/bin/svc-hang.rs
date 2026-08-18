//! svc-hang — 蓝图 40 收尾：心跳超时注入参考 EL0 服务（WP-40.4 隔离面
//! 实弹靶，蓝图 §9 矩阵 "isolation | AI worker OOM/hang | supervisor 终止"）。
//!
//! 与 svc-fault（fault 注入参考）同模式的演示服务。**不用 chunked 协议**
//! （chunked 的 Final 是同步 Call——卡死后调用方永久阻塞）；本服务收
//! 任意单向 Send 帧：正常帧回 pong 并心跳，`hang` 帧进入死循环
//! （无 fault、心跳停止）——root 侧 supervisor 心跳超时监控检测
//! "发过请求却无进展" → suspend + restart（不拖垮控制服务）。
//!
//! readiness：init 后 Signal 就绪 ntfn（与其它服务同契约）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_control_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_drv_svc::allocator::HeapAllocator =
    eneros_drv_svc::allocator::HeapAllocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    _eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    use eneros_sel4_sys::safe::{
        get_mr, ipc_recv, ipc_reply_recv, ipc_signal, message_info_new, set_mr,
    };
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // readiness（root 侧 PRODUCT-READY 收集）。
    let _ = ipc_signal(ready_ntfn);
    eneros_runtime::println!("[svc-hang] ready (heartbeat slot={heartbeat_slot})");
    let mut sender = 0u64;
    let mut reply_info = 0u64;
    loop {
        // 接收任意帧（Call 帧的回复经 reply_recv 送回；Send 帧回复被
        // 内核丢弃——root 以单向 Send 注入，永不阻塞）。
        let r = if reply_info == 0 {
            ipc_recv(ep, Some(&mut sender))
        } else {
            ipc_reply_recv(ep, reply_info, Some(&mut sender))
        };
        let Ok(mi) = r else {
            loop {
                core::hint::spin_loop();
            }
        };
        // 读取 MR[0..] 载荷（QEMU 实弹语义：数据区从 MR[0] 开始；
        // 按接收 length（words）截断——固定读满会含残留字节致比较失败）。
        use eneros_sel4_sys::types::message_info_length;
        let msg_words = message_info_length(mi) as usize;
        let mut payload = [0u8; 16];
        let mut len = 0usize;
        for i in 0..core::cmp::min(msg_words, 2) {
            if let Ok(word) = get_mr(i as u64) {
                let bytes = word.to_le_bytes();
                let n = core::cmp::min(8, payload.len() - len);
                payload[len..len + n].copy_from_slice(&bytes[..n]);
                len += n;
            }
        }
        if &payload[..len] == b"hang" {
            // 注入点：死循环（无 fault——**不 signal 心跳**，supervisor
            // 心跳超时 kill；判定先于心跳——注入轮无进展，probe 不解除）。
            eneros_runtime::println!(
                "[svc-hang] HANG injected — spinning (no fault, no heartbeat)"
            );
            loop {
                core::hint::spin_loop();
            }
        }
        // 心跳：正常消息处理后才 signal（进展证明；hang 帧不心跳）。
        if heartbeat_slot != 0 {
            let _ = ipc_signal(heartbeat_slot);
        }
        // 回 pong（MR[0]；Call 帧；Send 帧的回复被内核丢弃）。
        let _ = set_mr(0, u64::from_le_bytes(*b"pong\0\0\0\0"));
        reply_info = match message_info_new(0, 0, 0, 1) {
            Ok(mi) => mi,
            Err(_) => loop {
                core::hint::spin_loop();
            },
        };
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
