//! 跨 crate 测试支持（蓝图 39 WP-39.11：插件 host 测试共用回环传输）。
//! mock feature 门控——release resolved graph 不含本模块。

use alloc::boxed::Box;

use crate::transport::{
    TransportError, TransportFactory, TransportHandle, TransportKind, TransportSpec,
};

/// 测试用回环传输（echo：recv 返回最近一次 send 的数据）。
#[derive(Default)]
pub struct EchoTransport {
    last: core::cell::RefCell<alloc::vec::Vec<u8>>,
    connected: core::cell::Cell<bool>,
}

impl EchoTransport {
    pub fn new() -> Self {
        Self {
            last: core::cell::RefCell::new(alloc::vec::Vec::new()),
            connected: core::cell::Cell::new(true),
        }
    }
}

impl TransportHandle for EchoTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        *self.last.borrow_mut() = data.to_vec();
        Ok(())
    }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let data = self.last.borrow().clone();
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }
    fn close(&mut self) -> Result<(), TransportError> {
        self.connected.set(false);
        Ok(())
    }
    fn is_connected(&self) -> bool {
        self.connected.get()
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

/// 测试用工厂（只支持 Serial；open 返回 EchoTransport）。
#[derive(Default)]
pub struct EchoFactory;

impl TransportFactory for EchoFactory {
    fn open(&mut self, spec: &TransportSpec) -> Result<Box<dyn TransportHandle>, TransportError> {
        match spec.kind {
            TransportKind::Serial => Ok(Box::new(EchoTransport::new())),
            _ => Err(TransportError::Unsupported),
        }
    }
    fn supports(&self, kind: TransportKind) -> bool {
        kind == TransportKind::Serial
    }
}
