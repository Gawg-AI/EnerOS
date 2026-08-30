//! 期望通过：外部对 CertifiedPlan 的合法面 = decode + verify 绑定。
use eneros_decision_contract::CertifiedPlan;

fn main() {
    // CERT magic + V1 + 最小合法布局（decode 校验失败是运行时行为，
    // 编译期只证明 API 公开面存在且可调用）。
    let mut wire = vec![b'C', b'E', b'R', b'T', 0x01, 0x00, 0x00, 0x00];
    wire.extend_from_slice(&[0u8; 200]);
    let _ = CertifiedPlan::decode_v1(&wire);
}
