//! ChunkedSession 复制计数器违纪形态：两个会话**同时**可变借用同一共享
//! epoch 计数器 = E0499（endpoint.rs:131"禁止为绕借用检查复制计数器"的
//! 编译期执法——顺序复用同一 `&mut u64` 是唯一合规形态）。
use eneros_agent_sdk::chunked::RawIpc;
use eneros_agent_sdk::endpoint::ChunkedSession;

struct Dummy;

impl RawIpc for Dummy {
    fn request(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ns: u64,
    ) -> Result<std::vec::Vec<u8>, eneros_agent_sdk::AgentError> {
        Ok(Vec::new())
    }
    fn send(&mut self, _target: &str, _payload: &[u8]) -> Result<(), eneros_agent_sdk::AgentError> {
        Ok(())
    }
}

fn main() {
    let mut shared: u64 = 0;
    let mut enqueue = ChunkedSession::wrap_shared(Dummy, &mut shared);
    // 期望失败：第二个会话同时借用 shared——E0499（复制计数器绕借用
    // 检查的违纪形态在编译期不可表达）。
    let mut flush = ChunkedSession::wrap_shared(Dummy, &mut shared);
    let _ = (&mut enqueue, &mut flush);
}
