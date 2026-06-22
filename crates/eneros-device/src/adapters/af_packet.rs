//! AF_PACKET Transport — Linux 原始套接字，用于 GOOSE/SV 协议的 Layer 2 以太网直采。
//!
//! GOOSE（IEC 61850-8-1, EtherType 0x88B8）与 SV（IEC 61850-9-2, EtherType 0x88BA）
//! 均直接承载于以太网帧之上，不经过 IP/TCP/UDP 协议栈。本模块通过 Linux AF_PACKET
//! 原始套接字（`SOCK_RAW`）在指定网卡上收发完整以太网帧，实现亚毫秒级事件传输与
//! 4kHz 采样值直采。
//!
//! # 平台支持
//!
//! - **Linux**：使用 `libc` 直接调用 `socket`/`ioctl`/`bind`/`recvfrom`/`sendto`，
//!   并通过 `tokio::io::unix::AsyncFd` 将原生 fd 集成到 tokio 异步运行时。
//! - **非 Linux**：编译为 stub，`AfPacketTransport::new` 返回 `AdapterError::Unsupported`，
//!   保证跨平台编译通过（测试与帧编解码辅助函数仍可用）。
//!
//! # 权限要求
//!
//! 创建 `AF_PACKET + SOCK_RAW` 套接字需要 `CAP_NET_RAW` 能力（通常以 root 运行或
//! 通过 `setcap cap_net_raw+ep` 赋予）。
//!
//! # 与 GooseTransport 的关系
//!
//! `AfPacketTransport` 实现了 [`crate::adapters::goose::GooseTransport`] trait，
//! 因此可同时作为 [`crate::adapters::goose::GooseAdapter`] 与
//! [`crate::adapters::sv::SvAdapter`] 的传输层（SV 复用 GooseTransport 抽象）。

use crate::adapters::goose::GOOSE_ETHERTYPE;
use crate::adapters::sv::SV_ETHERTYPE;

/// AF_PACKET 传输错误类型。
///
/// 注：本类型定义于 af_packet 模块内（项目此前无统一 `AdapterError`），
/// 用于 `AfPacketTransport::new` 的构造期错误。`GooseTransport` trait 的
/// 运行期错误仍按 trait 约定使用 `String`。
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// 平台不支持（非 Linux 环境）
    #[error("unsupported platform: {0}")]
    Unsupported(String),
    /// 系统调用 I/O 错误
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// 指定的网络接口不存在
    #[error("interface not found: {0}")]
    InterfaceNotFound(String),
}

/// AF_PACKET 传输配置。
#[derive(Debug, Clone)]
pub struct AfPacketConfig {
    /// 网络接口名，如 "eth0"
    pub interface: String,
    /// 过滤的 EtherType（GOOSE=0x88B8, SV=0x88BA）
    pub ethertype: u16,
    /// 源 MAC 地址（发送时填入以太网帧源地址字段）
    pub src_mac: [u8; 6],
}

impl AfPacketConfig {
    /// 创建 GOOSE 传输配置（EtherType 0x88B8）。
    pub fn for_goose(interface: impl Into<String>, src_mac: [u8; 6]) -> Self {
        Self {
            interface: interface.into(),
            ethertype: GOOSE_ETHERTYPE,
            src_mac,
        }
    }

    /// 创建 SV 传输配置（EtherType 0x88BA）。
    pub fn for_sv(interface: impl Into<String>, src_mac: [u8; 6]) -> Self {
        Self {
            interface: interface.into(),
            ethertype: SV_ETHERTYPE,
            src_mac,
        }
    }
}

// ============================================================================
// 以太网帧构建/解析（跨平台共享，不依赖任何系统调用）
// ============================================================================

/// 以太网帧头长度：目的 MAC(6) + 源 MAC(6) + EtherType(2) = 14 字节。
pub const ETH_HEADER_LEN: usize = 14;

/// 构建以太网帧：`dst_mac + src_mac + ethertype(BE) + payload`。
pub fn build_ethernet_frame(
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    ethertype: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut frame = Vec::with_capacity(ETH_HEADER_LEN + payload.len());
    frame.extend_from_slice(dst_mac);
    frame.extend_from_slice(src_mac);
    frame.extend_from_slice(&ethertype.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// 解析以太网帧，返回 `(dst_mac, src_mac, ethertype, payload)`。
///
/// 帧长度不足 14 字节时返回 `None`。返回的切片均借用自 `data`。
#[allow(clippy::type_complexity)]
pub fn parse_ethernet_frame(
    data: &[u8],
) -> Option<(&[u8; 6], &[u8; 6], u16, &[u8])> {
    if data.len() < ETH_HEADER_LEN {
        return None;
    }
    let dst_mac: &[u8; 6] = data[0..6].try_into().ok()?;
    let src_mac: &[u8; 6] = data[6..12].try_into().ok()?;
    let ethertype = u16::from_be_bytes([data[12], data[13]]);
    let payload = &data[ETH_HEADER_LEN..];
    Some((dst_mac, src_mac, ethertype, payload))
}

/// 按 EtherType 过滤：若帧头 EtherType 匹配则返回 payload（不含以太网头），
/// 否则返回 `None`。帧过短同样返回 `None`。
pub fn filter_by_ethertype(data: &[u8], ethertype: u16) -> Option<&[u8]> {
    let (_dst, _src, et, payload) = parse_ethernet_frame(data)?;
    if et == ethertype {
        Some(payload)
    } else {
        None
    }
}

// ============================================================================
// Linux 原生实现
// ============================================================================

#[cfg(target_os = "linux")]
mod sys {
    use super::{filter_by_ethertype, AdapterError, AfPacketConfig, ETH_HEADER_LEN};
    use async_trait::async_trait;
    use crate::adapters::goose::GooseTransport;
    use crate::timestamp::{ProtocolTimestamp, TimestampSource};
    use std::io;
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use tokio::io::unix::AsyncFd;

    /// 接收缓冲区大小：覆盖最大以太网帧（含 jumbo / oversized），与 libpcap 默认 snaplen 一致。
    const RECV_BUF_SIZE: usize = 65536;

    /// 控制消息缓冲区大小：足够容纳 SCM_TIMESTAMPNS（timespec = 16 字节）+ cmsg 头 + 对齐填充。
    const CMSG_BUF_SIZE: usize = 64;

    /// Linux AF_PACKET 原始套接字传输。
    ///
    /// 持有一个非阻塞的 `AF_PACKET + SOCK_RAW` 套接字，绑定到指定网卡，
    /// 通过 `AsyncFd` 集成进 tokio 运行时。内核按配置的 EtherType 过滤帧，
    /// 用户态再次过滤作为双重保险。
    pub struct AfPacketTransport {
        fd: AsyncFd<OwnedFd>,
        ifindex: i32,
        ethertype: u16,
        src_mac: [u8; 6],
        /// 复用的接收缓冲区，避免每帧分配。
        /// 使用 `Mutex` 包裹以支持 `&self` 的 `receive`/`recv_with_timestamp` 方法。
        recv_buf: tokio::sync::Mutex<Vec<u8>>,
    }

    impl AfPacketTransport {
        /// 创建并配置 AF_PACKET 原始套接字。
        ///
        /// 步骤：
        /// 1. `socket(AF_PACKET, SOCK_RAW, htons(ETH_P_ALL))`
        /// 2. `ioctl(SIOCGIFINDEX)` 获取接口索引
        /// 3. `bind()` 到 `sockaddr_ll`（指定接口）
        /// 4. `setsockopt(SO_TIMESTAMPNS)`（为时间戳任务铺路）
        /// 5. 设置非阻塞并包装为 `AsyncFd`
        pub fn new(config: AfPacketConfig) -> Result<Self, AdapterError> {
            // 1. 创建原始套接字
            // SAFETY: socket() 仅返回 fd 或 -1，无内存安全风险。
            let raw_fd: RawFd = unsafe {
                libc::socket(
                    libc::AF_PACKET,
                    libc::SOCK_RAW,
                    libc::htons(libc::ETH_P_ALL as u16) as libc::c_int,
                )
            };
            if raw_fd < 0 {
                return Err(AdapterError::Io(io::Error::last_os_error()));
            }
            // SAFETY: raw_fd 由 socket() 新建且尚未被 OwnedFd 管理，转移所有权安全。
            let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

            // 2. 获取接口索引
            let ifindex = get_ifindex(owned_fd.as_raw_fd(), &config.interface)
                .map_err(|e| AdapterError::InterfaceNotFound(format!("{}: {}", config.interface, e)))?;

            // 3. 绑定到接口
            bind_to_interface(owned_fd.as_raw_fd(), ifindex)?;

            // 4. 启用 SO_TIMESTAMPNS（为 Task 6 时间戳铺路）
            enable_timestampns(owned_fd.as_raw_fd())?;

            // 5. 设置非阻塞
            set_nonblocking(owned_fd.as_raw_fd())?;

            // 包装为 AsyncFd，集成进 tokio reactor
            let fd = AsyncFd::new(owned_fd)?;

            Ok(Self {
                fd,
                ifindex,
                ethertype: config.ethertype,
                src_mac: config.src_mac,
                recv_buf: tokio::sync::Mutex::new(vec![0u8; RECV_BUF_SIZE]),
            })
        }

        /// 已绑定的接口索引。
        pub fn ifindex(&self) -> i32 {
            self.ifindex
        }

        /// 配置的 EtherType。
        pub fn ethertype(&self) -> u16 {
            self.ethertype
        }

        /// 源 MAC 地址。
        pub fn src_mac(&self) -> &[u8; 6] {
            &self.src_mac
        }

        /// 接收一帧并附带内核时间戳（SO_TIMESTAMPNS）。
        ///
        /// 与 [`GooseTransport::receive`] 不同，本方法使用 `recvmsg` 读取内核
        /// 附带的 `SCM_TIMESTAMPNS` 控制消息，返回 `(帧数据, ProtocolTimestamp)`。
        /// 若内核未提供时间戳（控制消息缺失），回退到软件时间戳。
        ///
        /// # 性能说明
        ///
        /// `recvmsg` 相比 `recvfrom` 有微小的额外开销（控制消息拷贝），但对于
        /// GOOSE/SV 的亚毫秒级时间精度需求是必要的。在 4kHz SV 采样场景下，
        /// 每秒 4000 次调用的开销可忽略不计。
        pub async fn recv_with_timestamp(
            &self,
        ) -> Result<(Vec<u8>, ProtocolTimestamp), String> {
            loop {
                let ethertype = self.ethertype;
                // 控制消息缓冲区：每帧重新初始化，避免残留旧数据
                let mut cmsg_buf = [0u8; CMSG_BUF_SIZE];
                let cmsg_ptr = cmsg_buf.as_mut_ptr();
                let cmsg_cap = cmsg_buf.len();

                let mut guard = self.fd.readable().await.map_err(|e| e.to_string())?;
                // 锁定接收缓冲区，仅在同步 I/O 期间持有
                let mut recv_buf = self.recv_buf.lock().await;
                let buf_ptr = recv_buf.as_mut_ptr();
                let buf_len = recv_buf.len();
                match guard.try_io(|inner| {
                    let raw_fd = inner.get_ref().as_raw_fd();
                    // 构建 iovec 指向接收缓冲区
                    let mut iov = libc::iovec {
                        iov_base: buf_ptr as *mut libc::c_void,
                        iov_len: buf_len,
                    };
                    // 构建 msghdr，设置控制消息缓冲区以接收 SO_TIMESTAMPNS
                    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
                    msg.msg_iov = &mut iov;
                    msg.msg_iovlen = 1;
                    msg.msg_control = cmsg_ptr as *mut libc::c_void;
                    msg.msg_controllen = cmsg_cap;

                    // SAFETY: recvmsg 从 raw_fd 读取数据到 iov 指向的 recv_buf，
                    // 同时将控制消息写入 cmsg_buf。两者均为有效可写内存。
                    let n = unsafe { libc::recvmsg(raw_fd, &mut msg, 0) };
                    if n < 0 {
                        Err(io::Error::last_os_error())
                    } else if msg.msg_flags & libc::MSG_TRUNC != 0 {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "received frame truncated (exceeds buffer size)",
                        ))
                    } else {
                        // 从控制消息中提取 SCM_TIMESTAMPNS
                        let ts = extract_timestampns(&msg);
                        Ok((n as usize, ts))
                    }
                }) {
                    Ok(Ok((n, ts_opt))) => {
                        let frame = &recv_buf[..n];
                        // 按 EtherType 过滤：不匹配的帧丢弃并继续接收
                        if filter_by_ethertype(frame, ethertype).is_some() {
                            let ts = ts_opt.unwrap_or_else(ProtocolTimestamp::now);
                            return Ok((frame.to_vec(), ts));
                        }
                    }
                    Ok(Err(e)) if e.kind() == io::ErrorKind::WouldBlock => continue,
                    Ok(Err(e)) => return Err(e.to_string()),
                    Err(_) => continue, // 竞态：就绪已失效，重新等待
                }
            }
        }
    }

    #[async_trait]
    impl GooseTransport for AfPacketTransport {
        async fn receive(&self) -> std::result::Result<Vec<u8>, String> {
            loop {
                let ethertype = self.ethertype;

                let mut guard = self.fd.readable().await.map_err(|e| e.to_string())?;
                // 锁定接收缓冲区，仅在同步 I/O 期间持有
                let mut recv_buf = self.recv_buf.lock().await;
                let buf_ptr = recv_buf.as_mut_ptr();
                let buf_len = recv_buf.len();
                match guard.try_io(|inner| {
                    let raw_fd = inner.get_ref().as_raw_fd();
                    // SAFETY: recvfrom 写入 buf_ptr 指向的 recv_buf（容量 buf_len），
                    // 此时无其他引用访问该缓冲区；raw_fd 来自已打开的 AF_PACKET 套接字。
                    let n = unsafe {
                        libc::recvfrom(
                            raw_fd,
                            buf_ptr as *mut libc::c_void,
                            buf_len,
                            0,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                        )
                    };
                    if n < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(n as usize)
                    }
                }) {
                    Ok(Ok(n)) => {
                        let frame = &recv_buf[..n];
                        // 按 EtherType 过滤：不匹配的帧丢弃并继续接收
                        if filter_by_ethertype(frame, ethertype).is_some() {
                            return Ok(frame.to_vec());
                        }
                    }
                    Ok(Err(e)) if e.kind() == io::ErrorKind::WouldBlock => continue,
                    Ok(Err(e)) => return Err(e.to_string()),
                    Err(_) => continue, // 竞态：就绪已失效，重新等待
                }
            }
        }

        async fn send(&self, frame: &[u8]) -> std::result::Result<(), String> {
            if frame.len() < ETH_HEADER_LEN {
                return Err("frame too short for Ethernet header".to_string());
            }
            let dst_mac: [u8; 6] = frame[0..6]
                .try_into()
                .map_err(|_| "invalid destination MAC".to_string())?;
            let ifindex = self.ifindex;
            let ethertype = self.ethertype;

            loop {
                let mut guard = self.fd.writable().await.map_err(|e| e.to_string())?;
                let frame_ptr = frame.as_ptr() as *const libc::c_void;
                let frame_len = frame.len();
                match guard.try_io(|inner| {
                    let raw_fd = inner.get_ref().as_raw_fd();
                    let mut addr: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
                    addr.sll_family = libc::AF_PACKET as u16;
                    addr.sll_protocol = libc::htons(ethertype as u16);
                    addr.sll_ifindex = ifindex;
                    addr.sll_halen = 6;
                    addr.sll_addr[..6].copy_from_slice(&dst_mac);
                    // SAFETY: sendto 从 frame_ptr 读取 frame_len 字节（来自入参 frame）；
                    // raw_fd 来自已打开的 AF_PACKET 套接字；addr 为本栈帧局部变量。
                    let n = unsafe {
                        libc::sendto(
                            raw_fd,
                            frame_ptr,
                            frame_len,
                            0,
                            &addr as *const _ as *const libc::sockaddr,
                            std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
                        )
                    };
                    if n < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(n as usize)
                    }
                }) {
                    Ok(Ok(n)) => {
                        if n == frame.len() {
                            return Ok(());
                        }
                        return Err(format!("short send: {} of {} bytes", n, frame.len()));
                    }
                    Ok(Err(e)) if e.kind() == io::ErrorKind::WouldBlock => continue,
                    Ok(Err(e)) => return Err(e.to_string()),
                    Err(_) => continue,
                }
            }
        }
    }

    /// 通过 `ioctl(SIOCGIFINDEX)` 获取网卡接口索引。
    fn get_ifindex(fd: RawFd, interface: &str) -> io::Result<i32> {
        let name = interface.as_bytes();
        if name.len() >= libc::IFNAMSIZ {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface name too long",
            ));
        }
        // SAFETY: ifreq 被 zeroed 初始化，随后仅填充 ifr_name（定长数组）。
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        for (i, &b) in name.iter().enumerate() {
            ifr.ifr_name[i] = b as libc::c_char;
        }
        // SAFETY: fd 来自已打开的套接字；ifr 为可变局部变量，ioctl 读取 ifr_name 并写入 ifru。
        let ret = unsafe { libc::ioctl(fd, libc::SIOCGIFINDEX, &mut ifr) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: ioctl 成功后 ifru 联合体已由内核写入 ifru_ifindex 字段。
        let ifindex = unsafe { ifr.ifr_ifru.ifru_ifindex };
        Ok(ifindex)
    }

    /// 将套接字绑定到指定接口（`sockaddr_ll`）。
    fn bind_to_interface(fd: RawFd, ifindex: i32) -> io::Result<()> {
        // SAFETY: sockaddr_ll zeroed 初始化后仅填充标量字段。
        let mut addr: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        addr.sll_family = libc::AF_PACKET as u16;
        addr.sll_protocol = libc::htons(libc::ETH_P_ALL as u16);
        addr.sll_ifindex = ifindex;
        // SAFETY: fd 来自已打开的套接字；addr 为局部变量，按值传指针。
        let ret = unsafe {
            libc::bind(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// 启用 `SO_TIMESTAMPNS`，使内核为接收的帧附带纳秒级时间戳控制消息。
    fn enable_timestampns(fd: RawFd) -> io::Result<()> {
        let on: libc::c_int = 1;
        // SAFETY: fd 来自已打开的套接字；on 为局部 c_int，按值传指针。
        let ret = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TIMESTAMPNS,
                &on as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// 通过 `fcntl` 设置套接字为非阻塞模式。
    fn set_nonblocking(fd: RawFd) -> io::Result<()> {
        // SAFETY: F_GETFL 不修改 fd 状态，仅返回当前标志。
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: F_SETFL 设置文件状态标志，O_NONBLOCK 为合法标志。
        let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// 从 `msghdr` 的控制消息中提取 `SO_TIMESTAMPNS` 时间戳。
    ///
    /// 遍历 `recvmsg` 返回的辅助数据（cmsg），查找 `SOL_SOCKET + SCM_TIMESTAMPNS`，
    /// 从中读取 `timespec` 并转换为 [`ProtocolTimestamp`]。
    ///
    /// # Safety
    ///
    /// 调用者需保证 `msg` 指向有效的 `msghdr`，且其 `msg_control` 缓冲区在
    /// 调用期间保持有效。本函数仅读取控制消息，不修改任何数据。
    fn extract_timestampns(msg: &libc::msghdr) -> Option<ProtocolTimestamp> {
        // SAFETY: CMSG_FIRSTHDR 仅读取 msg 的 msg_control 和 msg_controllen 字段，
        // 返回指向第一个 cmsghdr 的指针（或 null）。
        let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(msg as *const libc::msghdr) };
        while !cmsg.is_null() {
            // SAFETY: cmsg 由 CMSG_FIRSTHDR/CMSG_NXTHDR 返回，指向 msg_control 缓冲区内
            // 的有效 cmsghdr 结构。此处仅读取 cmsg_level 和 cmsg_type 字段。
            let cmsg_ref = unsafe { &*cmsg };
            if cmsg_ref.cmsg_level == libc::SOL_SOCKET
                && cmsg_ref.cmsg_type == libc::SCM_TIMESTAMPNS
            {
                // 读取 timespec 数据
                let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
                // SAFETY: CMSG_DATA 返回 cmsg 数据部分的指针，对于 SCM_TIMESTAMPNS
                // 其长度为 sizeof(timespec)。copy_nonoverlapping 安全拷贝。
                let data_ptr = unsafe { libc::CMSG_DATA(cmsg) };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data_ptr as *const u8,
                        &mut ts as *mut libc::timespec as *mut u8,
                        std::mem::size_of::<libc::timespec>(),
                    );
                }
                return Some(ProtocolTimestamp::from_timespec(
                    ts,
                    TimestampSource::Kernel,
                ));
            }
            // SAFETY: CMSG_NXTHDR 读取 msg 和当前 cmsg，返回下一个 cmsghdr 指针（或 null）。
            cmsg = unsafe { libc::CMSG_NXTHDR(msg as *const libc::msghdr, cmsg) };
        }
        None
    }
}

#[cfg(target_os = "linux")]
pub use sys::AfPacketTransport;

// ============================================================================
// 非 Linux 平台 stub
// ============================================================================

#[cfg(not(target_os = "linux"))]
mod stub {
    use super::{AdapterError, AfPacketConfig};
    use crate::adapters::goose::GooseTransport;
    use async_trait::async_trait;

    /// 非 Linux 平台的 stub —— 无法创建真实 AF_PACKET 套接字。
    pub struct AfPacketTransport {
        _private: (),
    }

    impl AfPacketTransport {
        /// 非 Linux 平台始终返回 `Unsupported` 错误。
        pub fn new(_config: AfPacketConfig) -> Result<Self, AdapterError> {
            Err(AdapterError::Unsupported(
                "AF_PACKET requires Linux".into(),
            ))
        }
    }

    // 非 Linux 平台的 stub 实现 GooseTransport，使 sv.rs/goose.rs 的 with_af_packet
    // 方法能通过类型检查（实际调用时 new() 已返回 Err，不会到达 receive/send）。
    #[async_trait]
    impl GooseTransport for AfPacketTransport {
        async fn receive(&self) -> std::result::Result<Vec<u8>, String> {
            Err("AF_PACKET not supported on this platform".to_string())
        }

        async fn send(&self, _frame: &[u8]) -> std::result::Result<(), String> {
            Err("AF_PACKET not supported on this platform".to_string())
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use stub::AfPacketTransport;

// ============================================================================
// 单元测试（跨平台共享，使用 Mock transport，不依赖真实网络）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::goose::{
        GooseData, GooseFrame, GooseTransport, MockGooseTransport, GOOSE_ETHERTYPE,
    };
    use crate::adapters::sv::{SvFrame, SV_ETHERTYPE};

    #[test]
    fn test_build_parse_ethernet_roundtrip() {
        let dst = [0x01, 0x0C, 0xCD, 0x01, 0x00, 0x00];
        let src = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        let frame = build_ethernet_frame(&dst, &src, GOOSE_ETHERTYPE, &payload);
        assert_eq!(frame.len(), ETH_HEADER_LEN + payload.len());

        let (d, s, et, pl) = parse_ethernet_frame(&frame).expect("parse ok");
        assert_eq!(d, &dst);
        assert_eq!(s, &src);
        assert_eq!(et, GOOSE_ETHERTYPE);
        assert_eq!(pl, &payload);
    }

    #[test]
    fn test_parse_ethernet_too_short() {
        assert!(parse_ethernet_frame(&[0u8; 5]).is_none());
        assert!(parse_ethernet_frame(&[]).is_none());
    }

    #[test]
    fn test_parse_ethernet_exact_header() {
        // 恰好 14 字节：可解析，payload 为空
        let frame = build_ethernet_frame(&[0; 6], &[0; 6], 0x88B8, &[]);
        assert_eq!(frame.len(), ETH_HEADER_LEN);
        let (_d, _s, et, pl) = parse_ethernet_frame(&frame).expect("parse ok");
        assert_eq!(et, 0x88B8);
        assert!(pl.is_empty());
    }

    #[test]
    fn test_filter_by_ethertype_match() {
        let frame = build_ethernet_frame(&[0; 6], &[0; 6], GOOSE_ETHERTYPE, &[1, 2, 3]);
        let payload = filter_by_ethertype(&frame, GOOSE_ETHERTYPE).expect("should match");
        assert_eq!(payload, &[1, 2, 3]);
    }

    #[test]
    fn test_filter_by_ethertype_no_match() {
        // IPv4 EtherType，与 GOOSE 不匹配
        let frame = build_ethernet_frame(&[0; 6], &[0; 6], 0x0800, &[1, 2, 3]);
        assert!(filter_by_ethertype(&frame, GOOSE_ETHERTYPE).is_none());
    }

    #[test]
    fn test_filter_by_ethertype_short_frame() {
        assert!(filter_by_ethertype(&[0u8; 5], GOOSE_ETHERTYPE).is_none());
    }

    #[test]
    fn test_ethernet_frame_layout() {
        // 验证以太网帧字节布局：dst(6) + src(6) + ethertype(2,BE) + payload
        // 该布局是 GOOSE/SV 在 Layer 2 直采时的帧格式基础。
        let dst = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let src = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let frame = build_ethernet_frame(&dst, &src, 0x88B8, &[0x01, 0x02]);
        assert_eq!(&frame[0..6], &dst);
        assert_eq!(&frame[6..12], &src);
        assert_eq!(u16::from_be_bytes([frame[12], frame[13]]), 0x88B8);
        assert_eq!(&frame[14..], &[0x01, 0x02]);
    }

    #[test]
    fn test_for_goose_ethertype() {
        let cfg = AfPacketConfig::for_goose("eth0", [0; 6]);
        assert_eq!(cfg.ethertype, 0x88B8);
    }

    #[test]
    fn test_for_sv_ethertype() {
        let cfg = AfPacketConfig::for_sv("eth0", [0; 6]);
        assert_eq!(cfg.ethertype, 0x88BA);
    }

    #[test]
    fn test_afpacket_config_helpers() {
        let cfg = AfPacketConfig::for_goose("eth0", [0; 6]);
        assert_eq!(cfg.interface, "eth0");
        assert_eq!(cfg.ethertype, 0x88B8);

        let cfg2 = AfPacketConfig::for_sv("eth1", [1; 6]);
        assert_eq!(cfg2.interface, "eth1");
        assert_eq!(cfg2.ethertype, 0x88BA);
        assert_eq!(cfg2.src_mac, [1; 6]);
    }

    #[tokio::test]
    async fn test_mock_transport_send_receive() {
        // 使用 Mock transport 验证收发路径，不依赖真实网络
        let (transport, sender) = MockGooseTransport::new();
        let dst = [0x01, 0x0C, 0xCD, 0x01, 0x00, 0x00];
        let src = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let frame = build_ethernet_frame(&dst, &src, GOOSE_ETHERTYPE, &[0xCA, 0xFE]);

        // 注入一帧并接收
        sender.send(frame.clone()).await.unwrap();
        let received = transport.receive().await.expect("receive ok");
        assert_eq!(received, frame);

        // 发送一帧并验证已记录
        transport.send(&frame).await.expect("send ok");
        let sent = transport.sent_frames().await;
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], frame);
    }

    #[tokio::test]
    async fn test_mock_transport_with_goose_frame() {
        // 端到端：通过 mock transport 收发 GOOSE 帧，验证以太网头与 PDU 解析
        let (transport, sender) = MockGooseTransport::new();
        let gframe = GooseFrame {
            appid: 0x0001,
            gocb_ref: "IED1_LD0/LLN0$GO$gcb1".into(),
            time_allowed_to_live: 1000,
            dat_set: "IED1_LD0/LLN0$dsGeneric".into(),
            go_id: String::new(),
            t: 1700000000000,
            st_num: 1,
            sq_num: 0,
            simulation: false,
            conf_rev: 1,
            nds_com: false,
            num_dat_set_entries: 1,
            all_data: vec![GooseData::Bool(true)],
        };
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let bytes = gframe.serialize(&src_mac);
        sender.send(bytes.clone()).await.unwrap();

        let received = transport.receive().await.unwrap();
        // 验证以太网头 EtherType 为 GOOSE
        let (_d, _s, et, _pl) = parse_ethernet_frame(&received).expect("parse ethernet");
        assert_eq!(et, GOOSE_ETHERTYPE);
        // 按 GOOSE EtherType 过滤应返回 payload
        assert!(filter_by_ethertype(&received, GOOSE_ETHERTYPE).is_some());
        // 完整解析为 GOOSE 帧
        let parsed = GooseFrame::parse(&received).expect("goose parse");
        assert_eq!(parsed.appid, 0x0001);
        assert_eq!(parsed.gocb_ref, "IED1_LD0/LLN0$GO$gcb1");
        assert_eq!(parsed.all_data.len(), 1);
    }

    #[test]
    fn test_sv_frame_ethertype_filtering() {
        // SV 帧序列化后应能被 SV EtherType 过滤命中，且不被 GOOSE 过滤命中
        let sframe = SvFrame {
            appid: 0x4000,
            sv_id: "MU01".into(),
            smp_cnt: 100,
            conf_rev: 1,
            refr_tm: None,
            smp_rate: 4000,
            seq_data: vec![100, -100],
            asdus: Vec::new(),
        };
        let bytes = sframe.serialize(&[0; 6]);
        let (_d, _s, et, _pl) = parse_ethernet_frame(&bytes).expect("parse ethernet");
        assert_eq!(et, SV_ETHERTYPE);
        assert!(filter_by_ethertype(&bytes, SV_ETHERTYPE).is_some());
        assert!(filter_by_ethertype(&bytes, GOOSE_ETHERTYPE).is_none());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_afpacket_new_unsupported_on_nonlinux() {
        // 非 Linux 平台：构造应返回 Unsupported 错误
        let cfg = AfPacketConfig::for_goose("eth0", [0; 6]);
        let result = AfPacketTransport::new(cfg);
        assert!(matches!(result, Err(AdapterError::Unsupported(_))));
    }
}
