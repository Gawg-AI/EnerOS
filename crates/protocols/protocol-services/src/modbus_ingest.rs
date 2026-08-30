//! 蓝图 61 v0.314.30（[07] §2.2/§2.3）：Modbus TCP 接入 adapter。
//!
//! 只负责协议状态机与原始值（[07] §2.2 分界）：组计划（相邻寄存器按
//! 设备、功能码、连续地址合并读取）、事务执行、raw →
//! [`RawObservation`] 推送。mapping/工程量/单位/事件身份/去重/日志全部
//! 属于 [`eneros_ingestion_core::IngestionModule`]——本 adapter 推送的
//! `RawObservation` 携带未富化的寄存器原始值（I64），身份字段留零。
//!
//! 协作语义（[07] §2.3）：target 无 async runtime——显式状态机 =
//! 每 poll 最多执行 `budget.max_transactions` 个组读事务（每事务一次
//! 阻塞传输往返，有界）；未完成的组留待下个 tick（module 轮转下发）。
//!
//! 组异常语义：组读失败 → 组内全部点展开为 INVALID 品质事件
//! （`out.push`），不把整组假装成功（[07] §2.3）。

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use eneros_ingestion_core::{
    IngestionAdapter, PointPlan, PollBudget, PollOutcome, PushOutcome, RawObservation,
    RawObservationSink,
};
use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationValue};

/// 组合并的最大地址空洞（寄存器；跨洞大于此值拆组——避免读无关段）。
pub const MAX_MERGE_GAP: u16 = 4;

/// 单组最大字数（Modbus 协议上限 125）。
pub const MAX_GROUP_WORDS: u16 = 125;

/// 合并后的组读事务。
#[derive(Debug, Clone)]
struct GroupRead {
    slave_addr: u8,
    func_code: u8,
    start: u16,
    end: u16, // 含
    points: Vec<PointPlan>,
}

impl GroupRead {
    fn words(&self) -> u16 {
        self.end.saturating_sub(self.start).saturating_add(1)
    }
}

/// 计划组读（设备+功能码+连续地址合并；[07] §2.3）。
fn plan_groups(work: &[PointPlan]) -> Vec<GroupRead> {
    let mut addrs: Vec<&PointPlan> = work
        .iter()
        .filter(|p| p.func_code == 3 || p.func_code == 4)
        .collect();
    addrs.sort_by_key(|p| (p.slave_addr, p.func_code, p.reg_addr));
    let mut groups: Vec<GroupRead> = Vec::new();
    for p in addrs {
        match groups.last_mut() {
            Some(g)
                if g.slave_addr == p.slave_addr
                    && g.func_code == p.func_code
                    && p.reg_addr.saturating_sub(g.end) <= MAX_MERGE_GAP
                    && p.reg_addr.saturating_sub(g.start) < MAX_GROUP_WORDS =>
            // span 上限 = 组内最大跨度 < MAX_GROUP_WORDS（与 words() 同界）
            {
                g.end = g.end.max(p.reg_addr);
                g.points.push(*p);
            }
            _ => {
                groups.push(GroupRead {
                    slave_addr: p.slave_addr,
                    func_code: p.func_code,
                    start: p.reg_addr,
                    end: p.reg_addr,
                    points: alloc::vec![*p],
                });
            }
        }
    }
    groups
}

/// 共享插件句柄（读=adapter 组读；写=bin 出站写——单线程 EL0，RefCell）。
pub type SharedPlugin = Rc<RefCell<eneros_modbus_tcp::plugin::TcpPlugin>>;

/// Modbus TCP 接入 adapter（bin 装配；传输在 [`eneros_modbus_tcp::plugin::TcpPlugin`]）。
pub struct ModbusIngestAdapter {
    source_id: u32,
    plugin: SharedPlugin,
    /// 自上次成功读以来累计重连计数（bin 侧折进 module.note_reconnect）。
    reconnects: u32,
    last_read_ok: bool,
    /// 本轮采集证据行计数（ACQ good/bad——bin 打印聚合）。
    pub acq_good: u32,
    pub acq_bad: u32,
}

impl ModbusIngestAdapter {
    pub fn new(source_id: u32, plugin: SharedPlugin) -> Self {
        Self {
            source_id,
            plugin,
            reconnects: 0,
            last_read_ok: true,
            acq_good: 0,
            acq_bad: 0,
        }
    }
}

impl IngestionAdapter for ModbusIngestAdapter {
    fn source_id(&self) -> u32 {
        self.source_id
    }

    fn poll(
        &mut self,
        now_ns: u64,
        budget: PollBudget,
        work: &[PointPlan],
        out: &mut dyn RawObservationSink,
    ) -> PollOutcome {
        let groups = plan_groups(work);
        let mut transactions = 0u32;
        let mut pushed = 0u32;
        for g in &groups {
            if transactions >= budget.max_transactions {
                return PollOutcome::Busy { transactions };
            }
            transactions += 1;
            let read = self
                .plugin
                .borrow_mut()
                .read_group(g.start, g.func_code, g.words());
            match read {
                Ok(regs) => {
                    if !self.last_read_ok {
                        self.reconnects = self.reconnects.saturating_add(1);
                        self.last_read_ok = true;
                    }
                    for p in &g.points {
                        let idx = (p.reg_addr - g.start) as usize;
                        let Some(&word) = regs.get(idx) else {
                            // 组响应短缺：该点按坏质量展开（不假装成功）。
                            let outcome = self.push_bad(out, p, now_ns);
                            if matches!(outcome, PushOutcome::Accepted | PushOutcome::Coalesced) {
                                pushed += 1;
                            }
                            if outcome == PushOutcome::Backpressured {
                                return PollOutcome::Busy { transactions };
                            }
                            continue;
                        };
                        let outcome = out.push(RawObservation {
                            point_id: p.point_id,
                            value: ObservationValue::I64(i64::from(word)),
                            event_time_ns: Some(now_ns.max(1)),
                            quality_bits: quality_bits::VALID,
                            integrity: IntegrityTag::LocalTrustedEdge,
                            flags: 0,
                            schema_id: 0,
                            schema_version: 0,
                            unit_code: 0,
                            asset_id: 0,
                            mapping_version: 0,
                            device_id: p.device_id,
                        });
                        match outcome {
                            PushOutcome::Backpressured => {
                                // sink 满：停推（[07] §2.5 adapter 合同——
                                // 继续推只刷限流计数与有界拒绝日志）。
                                self.acq_bad += 1;
                                return PollOutcome::Busy { transactions };
                            }
                            PushOutcome::Rejected => {
                                // 结构化拒绝按点计数，不吞组内其余点。
                                self.acq_bad += 1;
                            }
                            _ => {
                                self.acq_good += 1;
                                pushed += 1;
                            }
                        }
                    }
                }
                Err(_e) => {
                    self.last_read_ok = false;
                    for p in &g.points {
                        let outcome = self.push_bad(out, p, now_ns);
                        if matches!(outcome, PushOutcome::Accepted | PushOutcome::Coalesced) {
                            pushed += 1;
                        }
                        if outcome == PushOutcome::Backpressured {
                            // sink 满：停推（合同同上；余点下轮重排）。
                            return PollOutcome::Busy { transactions };
                        }
                    }
                }
            }
        }
        PollOutcome::Completed {
            transactions,
            events: pushed,
        }
    }

    fn take_reconnects(&mut self) -> u32 {
        core::mem::take(&mut self.reconnects)
    }
}

impl ModbusIngestAdapter {
    /// 组异常/短缺 → 该点坏质量事件（INVALID；死区永不抑制品质变化）。
    /// 统一计 acq_bad（坏采集——含已入流的坏质量展开）；返回原始回执
    /// （Backpressured = sink 满，调用方必须停推——[07] §2.5 adapter
    /// 合同）。
    fn push_bad(
        &mut self,
        out: &mut dyn RawObservationSink,
        p: &PointPlan,
        now_ns: u64,
    ) -> PushOutcome {
        let outcome = out.push(RawObservation {
            point_id: p.point_id,
            value: ObservationValue::I64(0),
            event_time_ns: Some(now_ns.max(1)),
            quality_bits: quality_bits::INVALID,
            integrity: IntegrityTag::LocalTrustedEdge,
            flags: 0,
            schema_id: 0,
            schema_version: 0,
            unit_code: 0,
            asset_id: 0,
            mapping_version: 0,
            device_id: p.device_id,
        });
        self.acq_bad += 1;
        outcome
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    /// 无传输 plugin（未 start）——read_group 必失败 → 组展开坏质量。
    fn dead_adapter(source_id: u32) -> ModbusIngestAdapter {
        ModbusIngestAdapter::new(
            source_id,
            Rc::new(RefCell::new(eneros_modbus_tcp::plugin::TcpPlugin::new())),
        )
    }

    fn wp(point: u32, reg: u16, fc: u8, slave: u8) -> PointPlan {
        PointPlan {
            source_id: 2,
            point_id: point,
            device_id: 2,
            period_ns: 1_000_000_000,
            phase_offset_ns: 0,
            deadline_ns: 2_000_000_000,
            deadband_abs: 0.0,
            deadband_rel: 0.0,
            qos: eneros_ingestion_core::EventQos::Q1,
            suppressible: true,
            timeout_ns: 1_000_000,
            max_retries: 2,
            schema_id: 1,
            schema_version: 1,
            unit_code: 0,
            asset_id: 1,
            mapping_version: 1,
            scale: 1.0,
            offset: 0.0,
            func_code: fc,
            reg_addr: reg,
            slave_addr: slave,
        }
    }

    #[derive(Default)]
    struct CaptureSink {
        pushes: Vec<(u32, u16, bool)>, // (point,INVALID?) — bool=坏质量
        /// push 回执策略（默认 Accepted；栈序注入背压/拒绝形态）。
        verdict: Vec<PushOutcome>,
    }

    impl RawObservationSink for CaptureSink {
        fn push(&mut self, raw: RawObservation) -> PushOutcome {
            let v = self.verdict.pop().unwrap_or(PushOutcome::Accepted);
            let bad = raw.quality_bits & quality_bits::INVALID != 0;
            self.pushes.push((raw.point_id, value_int(&raw.value), bad));
            v
        }
    }

    fn value_int(v: &ObservationValue) -> u16 {
        match v {
            ObservationValue::I64(x) => *x as u16,
            _ => 0,
        }
    }

    #[test]
    fn group_plan_merges_contiguous_same_slave_fc() {
        let work = alloc::vec![
            wp(3, 4010, 3, 1),
            wp(1, 4008, 3, 1),
            wp(2, 4009, 3, 1),
            wp(4, 4020, 4, 1), // 不同功能码 → 拆组
            wp(5, 4012, 3, 1), // 空洞 1（4010→4012 gap=2 ≤4）→ 并
            wp(6, 5000, 3, 2), // 不同从站 → 拆组
            wp(7, 6000, 3, 1), // 空洞 990 → 拆组
        ];
        let groups = plan_groups(&work);
        assert_eq!(
            groups.len(),
            4,
            "fc3@slave1 远洞拆两组 / fc4@slave1 / fc3@slave2"
        );
        assert_eq!(groups[0].start, 4008);
        assert_eq!(groups[0].end, 4012);
        assert_eq!(groups[0].points.len(), 4, "连续+小空洞合并");
        assert_eq!(groups[1].start, 6000, "大空洞拆组");
        assert_eq!(groups[2].func_code, 4, "不同功能码拆组");
        assert_eq!(groups[3].slave_addr, 2, "不同从站拆组");
    }

    #[test]
    fn group_failure_expands_to_invalid_per_point() {
        let mut a = dead_adapter(2);
        let work = alloc::vec![wp(1, 4000, 3, 1), wp(2, 4001, 3, 1)];
        let mut sink = CaptureSink::default();
        let out = a.poll(
            1_000,
            PollBudget {
                max_events: 16,
                max_transactions: 16,
            },
            &work,
            &mut sink,
        );
        assert_eq!(
            out,
            PollOutcome::Completed {
                transactions: 1,
                events: 2
            },
            "单组 1 事务；events = 实际推送（2 点坏质量展开）"
        );
        assert_eq!(sink.pushes.len(), 2, "组内全部点展开");
        assert!(sink.pushes.iter().all(|&(_, _, bad)| bad), "全部坏质量");
        assert_eq!(a.acq_bad, 2);
    }

    #[test]
    fn budget_limits_transactions_per_poll() {
        let mut a = dead_adapter(2);
        // 三组（不同从站）；预算 2 → Busy 前两组执行。
        let work = alloc::vec![wp(1, 4000, 3, 1), wp(2, 5000, 3, 2), wp(3, 6000, 3, 3),];
        let mut sink = CaptureSink::default();
        let out = a.poll(
            1_000,
            PollBudget {
                max_events: 64,
                max_transactions: 2,
            },
            &work,
            &mut sink,
        );
        assert_eq!(out, PollOutcome::Busy { transactions: 2 });
        assert_eq!(sink.pushes.len(), 2);
    }

    /// 背压停推（[07] §2.5 adapter 合同）：sink 满后立即停推返回 Busy
    /// ——不把组内其余点继续推向已满 sink（限流计数与有界拒绝日志不被
    /// 噪音冲刷）；余点已挂 pending，下轮重排/期限超限可见。
    #[test]
    fn backpressure_stops_push_mid_group() {
        let mut a = dead_adapter(2);
        let work = alloc::vec![wp(1, 4000, 3, 1), wp(2, 4001, 3, 1), wp(3, 4002, 3, 1)];
        // pop 自栈顶：第 1/2 次接受、第 3 次背压。
        let mut sink = CaptureSink {
            pushes: alloc::vec![],
            verdict: alloc::vec![
                PushOutcome::Backpressured,
                PushOutcome::Accepted,
                PushOutcome::Accepted
            ],
        };
        let out = a.poll(
            1_000,
            PollBudget {
                max_events: 64,
                max_transactions: 16,
            },
            &work,
            &mut sink,
        );
        assert_eq!(out, PollOutcome::Busy { transactions: 1 });
        assert_eq!(sink.pushes.len(), 3, "第 3 点背压即停——第 4 次推不存在");
        assert_eq!(a.acq_bad, 3, "坏采集统一计 acq_bad");
        assert_eq!(a.acq_good, 0);
    }
}
