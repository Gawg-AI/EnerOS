//! 主站写核（蓝图 66 WP-66.3.3，自 bin `svc-modbus-tcp` 整体迁入）。
//!
//! `master_write_v2` = digest 门 → 点表解析 → 值别门 → 出站写 → 独立
//! readback 三态。判定序与证据行词形（MASTER-WRITE / MASTER-READBACK）
//! 逐字节不变——词形 golden 锁见本模块测试段（include_str 字面锁）。
//! bin 保留 `#[cfg(target_os = "none")]` 门内调用与本方法签名包装。
//!
//! **本核只执行**——授权已由 ControlDispatch 九步链完成（不双头授权）。

use eneros_control_gateway::PCS_SETPOINT_READBACK_TOL_KW;
use eneros_protocol_abstract::plugin::ProtocolPlugin as _;
use eneros_protocol_abstract::PointRegistry as _;

/// 47（WP-47.1 协议出站写）：执行主站写 + 独立 readback，返回 2B
/// `OutcomeFrame`。
pub fn master_write_v2(
    registry: &eneros_protocol_abstract::VecPointRegistry,
    plugin: &crate::modbus_ingest::SharedPlugin,
    device: u32,
    point: u32,
    value: eneros_telemetry_model::command::CommandValue,
    command_digest: [u8; 32],
) -> [u8; 2] {
    use eneros_control_gateway::wire::{OutcomeCode, OutcomeFrame};
    use eneros_protocol_abstract::write::semantic_command_digest;
    let computed = semantic_command_digest(device, point, value);
    if computed != command_digest {
        eneros_runtime::println!(
            "[svc-modbus-tcp] MASTER-WRITE device={device} point={point} digest-mismatch —— Denied(2)"
        );
        return OutcomeFrame::new(OutcomeCode::Denied, 2).encode();
    }
    // 68.2 卫生项（F18）：digest 门为 u32 全值口径，点表 device 为 u16——
    // 超域 device 不得经 `as u16` 折叠与点表误配（0x10001 → 1）。
    if device > u32::from(u16::MAX) {
        eneros_runtime::println!(
            "[svc-modbus-tcp] MASTER-WRITE device={device} device-range 超 u16 点表域 —— Denied(7)"
        );
        return OutcomeFrame::new(OutcomeCode::Denied, 7).encode();
    }
    let Some(mapping) = registry.resolve(point) else {
        eneros_runtime::println!(
            "[svc-modbus-tcp] MASTER-WRITE point={point} 无点表映射 —— Denied(7)"
        );
        return OutcomeFrame::new(OutcomeCode::Denied, 7).encode();
    };
    if mapping.device_id != device as u16 {
        eneros_runtime::println!(
            "[svc-modbus-tcp] MASTER-WRITE device={device} != 点表 device={} —— Denied(7)",
            mapping.device_id
        );
        return OutcomeFrame::new(OutcomeCode::Denied, 7).encode();
    }
    let now = crate::now_ms();
    let mut cd_hex = [0u8; 16];
    eneros_hex::encode_into_lower(&command_digest[..8], &mut cd_hex);
    let cd = core::str::from_utf8(&cd_hex).unwrap_or("?");
    let power = match value {
        eneros_telemetry_model::command::CommandValue::ActivePowerKw(v) => v,
        _ => {
            eneros_runtime::println!(
                "[svc-modbus-tcp] MASTER-WRITE point={point} 命令类别与模拟量寄存器不符 —— Denied(7)"
            );
            return OutcomeFrame::new(OutcomeCode::Denied, 7).encode();
        }
    };
    match plugin
        .borrow_mut()
        .write_point(now, mapping, eneros_upa_model::PointValue::Float(power))
    {
        Ok(()) => {
            eneros_runtime::println!(
                "[svc-modbus-tcp] MASTER-WRITE device={device} point={point} power={power} cd={cd} ACK"
            );
        }
        Err(e) => {
            eneros_runtime::println!(
                "[svc-modbus-tcp] MASTER-WRITE device={device} point={point} 写失败 {e:?} —— Denied(8)"
            );
            return OutcomeFrame::new(OutcomeCode::Denied, 8).encode();
        }
    }
    // 独立 readback（重读该点；值比对——不得回显刚写载荷）。
    match plugin.borrow_mut().read_point(now, mapping) {
        Ok(read) if read.ok => {
            let eng = mapping.to_engineering(read.raw);
            let ok = (eng - power).abs() <= PCS_SETPOINT_READBACK_TOL_KW.max(f64::EPSILON);
            let td = eneros_protocol_abstract::write::semantic_command_digest(
                device,
                point,
                eneros_telemetry_model::command::CommandValue::ActivePowerKw(eng),
            );
            let mut td_hex = [0u8; 16];
            eneros_hex::encode_into_lower(&td[..8], &mut td_hex);
            let tds = core::str::from_utf8(&td_hex).unwrap_or("?");
            eneros_runtime::println!(
                "[svc-modbus-tcp] MASTER-READBACK point={point} eng={eng} target={power} {} td={tds}",
                if ok { "VERIFIED" } else { "MISMATCH" }
            );
            if ok {
                OutcomeFrame::new(OutcomeCode::Applied, 0).encode()
            } else {
                OutcomeFrame::new(OutcomeCode::AppliedReadbackMismatch, 0).encode()
            }
        }
        _ => {
            eneros_runtime::println!(
                "[svc-modbus-tcp] MASTER-READBACK point={point} 不可得 —— AppliedReadbackUnavailable"
            );
            OutcomeFrame::new(OutcomeCode::AppliedReadbackUnavailable, 0).encode()
        }
    }
}

#[cfg(test)]
mod tests {
    use eneros_protocol_abstract::{ProtocolAddress, ProtocolPointMapping, VecPointRegistry};

    use super::*;

    // ---- 词形 golden 锁（R-66.d 先行；[00] §3.4 MASTER-WRITE 词形冻结）----

    /// MASTER-WRITE / MASTER-READBACK 证据行词形逐字面在位——任何漂移即红
    /// （include_str 字面锁；与 QEMU 判据行扫描同源词形）。
    #[test]
    fn master_write_word_forms_golden_locked() {
        let src = include_str!("master_write.rs");
        for needle in [
            "digest-mismatch —— Denied(2)",
            "无点表映射 —— Denied(7)",
            "!= 点表 device={} —— Denied(7)",
            "命令类别与模拟量寄存器不符 —— Denied(7)",
            "power={power} cd={cd} ACK",
            "写失败 {e:?} —— Denied(8)",
            "MASTER-READBACK point={point} eng={eng} target={power} {} td={tds}",
            "不可得 —— AppliedReadbackUnavailable",
        ] {
            assert!(
                src.contains(needle),
                "MASTER-WRITE 词形漂移（蓝图 66 §3.4 冻结）：{needle:?}"
            );
        }
    }

    // ---- 行为单测（host；蓝图 66 判据 ≥5 用例）----

    fn mapping(point_id: u32, device_id: u16) -> ProtocolPointMapping {
        ProtocolPointMapping {
            point_id,
            device_id,
            protocol_addr: ProtocolAddress::Modbus {
                slave_addr: device_id as u8,
                reg_addr: 4097,
                func_code: 3,
            },
            data_type: eneros_upa_model::PointType::Analog,
            scale: 0.1,
            offset: 0.0,
            identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
        }
    }

    fn digest_of(
        device: u32,
        point: u32,
        v: eneros_telemetry_model::command::CommandValue,
    ) -> [u8; 32] {
        eneros_protocol_abstract::write::semantic_command_digest(device, point, v)
    }

    #[test]
    fn digest_mismatch_denied_class2() {
        let reg = VecPointRegistry::new(vec![]);
        let plugin: crate::modbus_ingest::SharedPlugin = alloc::rc::Rc::new(
            core::cell::RefCell::new(eneros_modbus_tcp::plugin::TcpPlugin::new()),
        );
        let frame = master_write_v2(
            &reg,
            &plugin,
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(50.0),
            [0xAB; 32], // 与语义摘要不符
        );
        assert_eq!(frame, [3, 2], "digest-mismatch → Denied(2)");
    }

    #[test]
    fn unknown_point_denied_class7() {
        let reg = VecPointRegistry::new(vec![]); // 空点表
        let plugin: crate::modbus_ingest::SharedPlugin = alloc::rc::Rc::new(
            core::cell::RefCell::new(eneros_modbus_tcp::plugin::TcpPlugin::new()),
        );
        let d = digest_of(
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(50.0),
        );
        let frame = master_write_v2(
            &reg,
            &plugin,
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(50.0),
            d,
        );
        assert_eq!(frame, [3, 7], "无点表映射 → Denied(7)");
    }

    #[test]
    fn device_mismatch_denied_class7() {
        // 点表该点绑定 device 9，请求 device 1。
        let reg = VecPointRegistry::new(vec![mapping(100, 9)]);
        let plugin: crate::modbus_ingest::SharedPlugin = alloc::rc::Rc::new(
            core::cell::RefCell::new(eneros_modbus_tcp::plugin::TcpPlugin::new()),
        );
        let d = digest_of(
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(50.0),
        );
        let frame = master_write_v2(
            &reg,
            &plugin,
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(50.0),
            d,
        );
        assert_eq!(frame, [3, 7], "device != 点表 device → Denied(7)");
    }

    #[test]
    fn wrong_value_class_denied_class7() {
        let reg = VecPointRegistry::new(vec![mapping(100, 1)]);
        let plugin: crate::modbus_ingest::SharedPlugin = alloc::rc::Rc::new(
            core::cell::RefCell::new(eneros_modbus_tcp::plugin::TcpPlugin::new()),
        );
        // 值别门：SwitchPosition 值类别与模拟量寄存器不符（摘要与其同
        // 值类别一致，digest 门放行 → 值别门拒绝）。
        let v = eneros_telemetry_model::command::CommandValue::SwitchPosition(1);
        let d = digest_of(1, 100, v);
        let frame = master_write_v2(&reg, &plugin, 1, 100, v, d);
        assert_eq!(frame, [3, 7], "命令类别不符 → Denied(7)");
    }

    /// 68.2 卫生项（F18）：u32 device 折叠 u16 不得匹配点表——请求
    /// device=0x10001（`as u16` 折叠 = 1）与点表 device 1 同点时，digest
    /// 门（u32 全值口径）通过后不得因截断比较而放行。修复前：折叠匹配
    /// → 直达出站写（digest 门与点表门口径不一致）。
    #[test]
    fn t6_device_id_truncation_rejected() {
        let reg = VecPointRegistry::new(vec![mapping(100, 1)]);
        let plugin: crate::modbus_ingest::SharedPlugin = alloc::rc::Rc::new(
            core::cell::RefCell::new(eneros_modbus_tcp::plugin::TcpPlugin::new()),
        );
        let v = eneros_telemetry_model::command::CommandValue::ActivePowerKw(50.0);
        let d = digest_of(0x10001, 100, v);
        let frame = master_write_v2(&reg, &plugin, 0x10001, 100, v, d);
        assert_eq!(
            frame,
            [3, 7],
            "device 超 u16 点表域 → Denied(7)（折叠 1 不得匹配点表 device 1）"
        );
    }

    // ---- readback 三态（scripted 传输；FC06 回显应答 + FC03 定值应答）----

    use alloc::{boxed::Box, rc::Rc, vec, vec::Vec};
    use core::cell::RefCell;
    use std::collections::VecDeque;

    /// 脚本化传输：每交换弹出一个应答；`Echo` = FC06 回显请求自身；
    /// 队列空 → recv Err（readback 不可得形态）。
    struct ScriptedMasterHandle {
        script: RefCell<VecDeque<ScriptedResp>>,
        last_sent: RefCell<Vec<u8>>,
    }

    enum ScriptedResp {
        Echo,
        Frame(Vec<u8>),
    }

    impl eneros_protocol_abstract::TransportHandle for ScriptedMasterHandle {
        fn send(&mut self, data: &[u8]) -> Result<(), eneros_protocol_abstract::TransportError> {
            *self.last_sent.borrow_mut() = data.to_vec();
            Ok(())
        }
        fn recv(
            &mut self,
            buf: &mut [u8],
        ) -> Result<usize, eneros_protocol_abstract::TransportError> {
            let next = self
                .script
                .borrow_mut()
                .pop_front()
                .ok_or(eneros_protocol_abstract::TransportError::Timeout)?;
            let resp = match next {
                ScriptedResp::Echo => self.last_sent.borrow().clone(),
                ScriptedResp::Frame(f) => f,
            };
            let n = resp.len().min(buf.len());
            buf[..n].copy_from_slice(&resp[..n]);
            Ok(n)
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn close(&mut self) -> Result<(), eneros_protocol_abstract::TransportError> {
            Ok(())
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    struct ScriptedFactory {
        handle: Rc<RefCell<ScriptedMasterHandle>>,
    }

    impl eneros_protocol_abstract::TransportFactory for ScriptedFactory {
        fn open(
            &mut self,
            _spec: &eneros_protocol_abstract::TransportSpec,
        ) -> Result<
            Box<dyn eneros_protocol_abstract::TransportHandle>,
            eneros_protocol_abstract::TransportError,
        > {
            Ok(Box::new(SharedScriptHandle(self.handle.clone())))
        }
        fn supports(&self, _kind: eneros_protocol_abstract::TransportKind) -> bool {
            true
        }
    }

    /// 共享底层脚本（factory 每次 open 复用同一脚本状态）。
    struct SharedScriptHandle(Rc<RefCell<ScriptedMasterHandle>>);

    impl eneros_protocol_abstract::TransportHandle for SharedScriptHandle {
        fn send(&mut self, data: &[u8]) -> Result<(), eneros_protocol_abstract::TransportError> {
            self.0.borrow_mut().send(data)
        }
        fn recv(
            &mut self,
            buf: &mut [u8],
        ) -> Result<usize, eneros_protocol_abstract::TransportError> {
            self.0.borrow_mut().recv(buf)
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn close(&mut self) -> Result<(), eneros_protocol_abstract::TransportError> {
            Ok(())
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    /// 装配 scripted 插件：写交换（FC06 回显）+ 读交换（FC03 raw 应答）。
    fn scripted_plugin(read_raw: Option<i64>) -> crate::modbus_ingest::SharedPlugin {
        let mut script = VecDeque::new();
        script.push_back(ScriptedResp::Echo); // 写交换：FC06 回显
        if let Some(raw) = read_raw {
            // FC03 应答：MBAP(txn0 proto0 len5) unit1 func3 bc2 raw hi/lo。
            let hi = (raw >> 8) as u8;
            let lo = (raw & 0xFF) as u8;
            script.push_back(ScriptedResp::Frame(alloc::vec![
                0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x01, 0x03, 0x02, hi, lo,
            ]));
        } // None：读交换无脚本 → Timeout → readback 不可得
        let handle = Rc::new(RefCell::new(ScriptedMasterHandle {
            script: RefCell::new(script),
            last_sent: RefCell::new(Vec::new()),
        }));
        let mut plugin = eneros_modbus_tcp::plugin::TcpPlugin::new();
        let mut factory = ScriptedFactory { handle };
        let mut cfg = eneros_protocol_abstract::ProtocolConfig::default();
        cfg.insert("peer", "10.0.0.1:502");
        let mut reg = VecPointRegistry::new(vec![mapping(100, 1)]);
        plugin
            .start(&cfg, &mut factory, &mut reg)
            .expect("scripted start");
        Rc::new(RefCell::new(plugin))
    }

    const TOL: f64 = 5.0; // < control-gateway PCS_SETPOINT_READBACK_TOL_KW

    #[test]
    fn readback_verified_applied() {
        // scale 0.1 → eng = 4660*0.1 = 466.0；写入同值 → |Δ|≤TOL → Applied。
        let reg = VecPointRegistry::new(vec![mapping(100, 1)]);
        let plugin = scripted_plugin(Some(4660));
        let d = digest_of(
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(466.0),
        );
        let frame = master_write_v2(
            &reg,
            &plugin,
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(466.0),
            d,
        );
        assert!(
            frame[0] == 0 && frame[1] == 0,
            "VERIFIED → Applied(0)：{frame:?}"
        );
        let delta: f64 = (466.0_f64 - 4660.0_f64 * 0.1).abs();
        assert!(delta <= TOL.max(f64::EPSILON));
    }

    #[test]
    fn readback_mismatch_applied_readback_mismatch() {
        // 读出 eng=466.0 ≠ 写入 100.0 → AppliedReadbackMismatch。
        let reg = VecPointRegistry::new(vec![mapping(100, 1)]);
        let plugin = scripted_plugin(Some(4660));
        let d = digest_of(
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(100.0),
        );
        let frame = master_write_v2(
            &reg,
            &plugin,
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(100.0),
            d,
        );
        assert!(
            frame[0] == 5 && frame[1] == 0,
            "MISMATCH → AppliedReadbackMismatch：{frame:?}"
        );
    }

    #[test]
    fn readback_unavailable() {
        // 读交换无脚本（recv Timeout）→ AppliedReadbackUnavailable。
        let reg = VecPointRegistry::new(vec![mapping(100, 1)]);
        let plugin = scripted_plugin(None);
        let d = digest_of(
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(100.0),
        );
        let frame = master_write_v2(
            &reg,
            &plugin,
            1,
            100,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(100.0),
            d,
        );
        assert!(
            frame[0] == 4 && frame[1] == 0,
            "不可得 → AppliedReadbackUnavailable：{frame:?}"
        );
    }
}
