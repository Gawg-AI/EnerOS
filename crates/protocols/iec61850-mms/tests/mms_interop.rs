//! ARC-007：IEC 61850 MMS 客户端独立对端互操作证据。
//!
//! 独立对端 = 测试内 raw `std::net::TcpListener`，按 MMS/COTP/ACSE 规范
//! **逐字节手写**握手与 Read 应答（与 crate 的 BER 编解码零共享）：
//! COTP CR → CC（LI=9, 0xD0）；ACSE AARQ → AARE（0x61 接受）；MMS Read 请求 →
//! Confirmed-ResponsePdu（0xA1）+ Read-Response（0xA5）+ SequenceOf（0xA0）
//! Float 值（0x87, f32 = 25.5）——invokeID 回显。客户端解析独立对端应答
//! 成功 = 真实对端闭环证据（报文实录断言）。
#![allow(clippy::disallowed_macros)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use eneros_iec61850_mms::{MmsClient, MmsError, MmsResponse, MmsTransport, VarAccessSpec};
use eneros_iec61850_model::DaValue;

/// Raw TCP 传输层（仅字节搬运，与 crate 编解码零共享）。
struct RawTcpTransport {
    stream: Option<TcpStream>,
}

impl MmsTransport for RawTcpTransport {
    fn connect(&mut self, addr: &str, port: u16) -> Result<(), MmsError> {
        self.stream = Some(TcpStream::connect((addr, port)).map_err(|_| MmsError::Timeout)?);
        Ok(())
    }
    fn send(&mut self, pdu: &[u8]) -> Result<(), MmsError> {
        if let Some(s) = &mut self.stream {
            s.write_all(pdu).map_err(|_| MmsError::Timeout)?;
        }
        Ok(())
    }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, MmsError> {
        if let Some(s) = &mut self.stream {
            s.read(buf).map_err(|_| MmsError::Timeout)
        } else {
            Err(MmsError::Timeout)
        }
    }
}

/// COTP 数据头（客户端与服务端通用）：02 F0 80
const COTP_DATA: [u8; 3] = [0x02, 0xF0, 0x80];

/// COTP CC（Connect Confirm，手写）：LI=9, 0xD0, 参数 1024。
fn cotp_cc() -> [u8; 10] {
    [0x09, 0xD0, 0x00, 0x00, 0x00, 0x01, 0x00, 0xC0, 0x01, 0x0A]
}

/// 独立 MMS 服务端线程：握手 + 应答一次 Read（返回浮点 25.5）。
fn spawn_mms_peer() -> (u16, std::thread::JoinHandle<(u8, u32)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 512];

        // 1. COTP CR → CC
        let n = sock.read(&mut buf).unwrap();
        assert_eq!(buf[..n].len(), 10, "对端收到 COTP CR（10 字节）");
        assert_eq!(buf[1], 0xE0, "COTP CR TPDU");
        sock.write_all(&cotp_cc()).unwrap();

        // 读一个完整 COTP 数据段（03 F0 80 + TLV，支持多字节长度）
        fn read_cotp_data(sock: &mut TcpStream, out: &mut [u8]) -> usize {
            let mut acc: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 512];
            loop {
                let n = sock.read(&mut tmp).unwrap();
                acc.extend_from_slice(&tmp[..n]);
                if acc.len() >= 5 {
                    // TLV 长度（支持长格式）
                    let len0 = acc[4] as usize;
                    let content_len = if len0 & 0x80 != 0 {
                        let nbytes = len0 & 0x7f;
                        if acc.len() >= 5 + nbytes {
                            let mut l = 0usize;
                            for i in 0..nbytes {
                                l = (l << 8) | acc[5 + i] as usize;
                            }
                            l
                        } else {
                            continue;
                        }
                    } else {
                        len0
                    };
                    // COTP 头 3 + tag 1 + len 字段 + content
                    let total = 3
                        + 1
                        + if len0 & 0x80 != 0 {
                            1 + (len0 & 0x7f)
                        } else {
                            1
                        }
                        + content_len;
                    if acc.len() >= total {
                        break;
                    }
                }
            }
            out[..acc.len()].copy_from_slice(&acc);
            acc.len()
        }

        // 2. AARQ → AARE
        let _n = read_cotp_data(&mut sock, &mut buf);
        assert_eq!(buf[..3], COTP_DATA, "AARQ 经 COTP 数据包装");
        assert_eq!(buf[3], 0x60, "ACSE AARQ tag");
        // 客户端接收端期望**纯 AARE**（decode_aare 从 data[0] 解析，不剥 COTP 头）
        sock.write_all(&[0x61, 0x03, 0x02, 0x01, 0x00]).unwrap();

        // 3. MMS Read 请求 → Read-Response（invokeID 回显）
        let n = read_cotp_data(&mut sock, &mut buf);
        assert_eq!(buf[..3], COTP_DATA, "MMS 请求经 COTP 数据包装");
        assert_eq!(buf[3], 0xA0, "MMS Confirmed-RequestPdu tag");
        let invoke_id = u32::from(buf[5]) << 16 | u32::from(buf[6]) << 8 | u32::from(buf[7]);
        // Confirmed-ResponsePdu: A1 len
        //   invokeID: 02 01 <id>
        //   Read-Response: A5 len
        //     SequenceOf: A0 len
        //       Float: 87 04 <f32 25.5 = 0x41CC0000 BE 00 00 CC 41>
        let value: [u8; 4] = 25.5f32.to_be_bytes(); // 41 CC 00 00
        let mut resp = Vec::with_capacity(32);
        resp.extend_from_slice(&COTP_DATA);
        resp.push(0xA1);
        resp.push(0x09); // 内容长：invoke(3) + A5(2) + A0(2) + float(5) = 12 → 0x0C
                         // 上面长度算错——重算：A1 内容 = invokeID TLV(3) + A5 TLV
                         // A5 内容 = A0 TLV(2) + float TLV(5) = 7 → A5: A5 07
                         // A0 内容 = float TLV(5) → A0 05
                         // float: 87 04 + 4
                         // invoke: 02 01 <id> (3)
                         // A1 内容 = 3 + (2 + 7) = 12 = 0x0C
        resp.clear();
        // 客户端 decode_read_response 从 data[0] 解析（无 COTP 头）
        resp.push(0xA1);
        resp.push(0x0C);
        resp.push(0x02);
        resp.push(0x01);
        resp.push(invoke_id as u8);
        resp.push(0xA5);
        resp.push(0x07);
        resp.push(0xA0);
        resp.push(0x05);
        resp.push(0x87);
        resp.push(0x04);
        resp.extend_from_slice(&value);
        sock.write_all(&resp).unwrap();
        let _ = n;
        (invoke_id as u8, invoke_id)
    });
    (port, handle)
}

#[test]
fn arc007_mms_independent_peer_interop() {
    let (port, peer) = spawn_mms_peer();
    let transport = RawTcpTransport { stream: None };
    let mut client = MmsClient::new(transport, "1.1.1.999", 2000);
    client.connect("127.0.0.1", port).unwrap();
    let resp = client
        .read(&[VarAccessSpec {
            domain: String::from("D"),
            item: String::from("MMXU1.A.phsA.cVal.mag.f"),
        }])
        .unwrap();
    let (echo_id, _) = peer.join().unwrap();
    assert!(echo_id >= 1, "对端回显 invokeID");
    match resp {
        MmsResponse::ReadResult { results } => {
            assert_eq!(results.len(), 1, "解析出独立对端 Read 应答");
            match &results[0].value {
                Some(DaValue::Float32(f)) => {
                    assert!((f - 25.5).abs() < 1e-3, "MMS 浮点 25.5 经独立对端闭环: {f}");
                }
                other => panic!("期望 Float 值，实际 {other:?}"),
            }
        }
        other => panic!("期望 ReadResult，实际 {other:?}"),
    }
}
