//! 蓝图 68 卷 68.4：host 测试共享夹具（test-util 缝——objective-engine
//! WP-64.3.2.1 先例：dependent 的 dev-deps 启用本 feature，生产面零
//! feature 零 footprint）。
//!
//! [`FakeStorage`] = svc-storage 块命令面内存仿真 + 字节偏移记录器：
//! `EipcBlockDevice`/`BlockIo` 全链 host 注入面（真 wire 信封解码——协议
//! 偏差在测试边界即暴露）；`cmds` 记录 (cmd, off, len) 供越区段负例
//! （t3）对账。`Rc` 介面使"掉电"语义 = 弃驱动实例、介质长存可共享。

use alloc::rc::Rc;
// no_std 编译面（dependent dev-deps 启用时本 crate 无 test cfg）。
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use eneros_drv_svc::blk_wire;
use eneros_drv_svc::block_cmd::{BLK_CMD_FLUSH, BLK_CMD_READ, BLK_CMD_WRITE};
use eneros_drv_svc::runtime::{wire, DriverResponse, OP_EXECUTE, ST_OK};

#[derive(Clone)]
pub struct FakeStorage {
    disk: Rc<RefCell<Vec<u8>>>,
    cmds: Rc<RefCell<Vec<(u8, u64, usize)>>>,
}

impl FakeStorage {
    /// 全零介质（`region_bytes` 字节）。
    pub fn new(region_bytes: usize) -> Self {
        Self {
            disk: Rc::new(RefCell::new(vec![0u8; region_bytes])),
            cmds: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// 全部块命令 (cmd, byte_offset, len) 按序（越区段对账观测口）。
    pub fn cmds(&self) -> alloc::vec::Vec<(u8, u64, usize)> {
        self.cmds.borrow().clone()
    }
}

impl eneros_agent_sdk::block_io::EipcSend for FakeStorage {
    fn drv_call(&mut self, req: &[u8], _timeout_ns: u64) -> Result<Vec<u8>, ()> {
        let r = wire::decode_request(req).ok_or(())?;
        if r.opcode != OP_EXECUTE || r.payload_len == 0 {
            return Err(());
        }
        let sub = r.payload[0];
        let mut resp = DriverResponse::new(r.correlation, ST_OK, 0xA1);
        match sub {
            BLK_CMD_READ => {
                // WP-76.2.4：布局门单源（旧门是手写 `< 1+8+4` 偏移，且无 count
                // 域界——超 461B 越 `resp.payload` 界）。拒绝形态保持本替身惯例
                // = transport Err（其夹具以 expect_err 消费）。
                let gate = blk_wire::parse_read(blk_wire::body_of(&r))
                    .ok()
                    .filter(|(_, n)| *n > 0 && (*n as usize) <= blk_wire::BLK_READ_MAX);
                let Some((off, n)) = gate else {
                    return Err(());
                };
                let n = n as usize;
                self.cmds.borrow_mut().push((BLK_CMD_READ, off, n));
                let disk = self.disk.borrow();
                let end = off as usize + n;
                if end > disk.len() {
                    return Err(());
                }
                resp.payload[..n].copy_from_slice(&disk[off as usize..end]);
                resp.payload_len = n as u16;
            }
            BLK_CMD_WRITE => {
                let Ok((off, data)) = blk_wire::parse_write(blk_wire::body_of(&r)) else {
                    return Err(());
                };
                self.cmds
                    .borrow_mut()
                    .push((BLK_CMD_WRITE, off, data.len()));
                let mut disk = self.disk.borrow_mut();
                let end = off as usize + data.len();
                if end > disk.len() {
                    return Err(());
                }
                disk[off as usize..end].copy_from_slice(data);
            }
            BLK_CMD_FLUSH => {}
            _ => return Err(()),
        }
        Ok(wire::encode_response(&resp).to_vec())
    }
}
