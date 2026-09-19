//! F70-02（B07）：BOOTCTRL 记录补丁工具——在**不启动**的情况下推进
//! `prepare_attempt`（boot-manager 引导前扣减试启次数的单源逻辑）。
//!
//! 用途：QEMU 窗构造「最后一次试启」介质形态（tries=1 → 下次引导
//! prepare_attempt 1→0、本次仍运行被选槽 = S1 缺陷的精确介质前提）。
//! 补丁完全复用 `BootControl::prepare_attempt`（序列 +1 / tries −1 /
//! boot_attempts +1）——产物与真实引导产生的记录逐字节同构，经
//! boot-manager 锚的「运行时事务采纳」路径（sequence 严格高于锚）合法
//! 采纳，不绕过任何启动判据。
//!
//! 双副本处理：取高序列有效副本为基线，连续推进 N 次，双副本同字节
//! 写回（与运行时事务写序的最终态一致）。
//!
//! 用法：`cargo xtask bootctrl-prepare-attempt --image <raw> --times 2`

use std::fs;
use std::path::Path;

use eneros_boot_protocol::boot_control::{BootControl, BOOT_CONTROL_RECORD_SIZE};

pub fn command_bootctrl_prepare_attempt(root: &Path, args: &[String]) -> super::DynResult<()> {
    let mut image: Option<String> = None;
    let mut times: u64 = 1;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| super::invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--image" => image = Some(val()?),
            "--times" => {
                times = val()?
                    .parse()
                    .map_err(|_| super::invalid(String::from("--times must be an integer")))?;
            }
            other => return Err(super::invalid(format!("unknown bootctrl arg '{other}'"))),
        }
    }
    let image = image.ok_or_else(|| super::invalid(String::from("--image is required")))?;
    let path = root.join(image);
    let bytes =
        fs::read(&path).map_err(|e| super::invalid(format!("read {}: {e}", path.display())))?;
    if bytes.len() < BOOT_CONTROL_RECORD_SIZE {
        return Err(super::invalid(String::from(
            "image too small for a BOOTCTRL record",
        )));
    }
    // qemu-v1 布局：BOOTCTRL 分区基 lba 18432（boot_disk 单源）。
    let (bc_lba, _) =
        eneros_ota_ab::boot_disk::lba_range_of(eneros_ota_ab::boot_disk::BootPartition::BootCtrl);
    let base = (bc_lba as usize) * 512;
    if base + BOOT_CONTROL_RECORD_SIZE > bytes.len() {
        return Err(super::invalid(String::from(
            "image too small for BOOTCTRL partition",
        )));
    }
    let second_off = base + BOOT_CONTROL_RECORD_SIZE;
    let mut first = [0u8; BOOT_CONTROL_RECORD_SIZE];
    first.copy_from_slice(&bytes[base..base + BOOT_CONTROL_RECORD_SIZE]);
    let mut second = [0u8; BOOT_CONTROL_RECORD_SIZE];
    second.copy_from_slice(&bytes[second_off..second_off + BOOT_CONTROL_RECORD_SIZE]);
    let d0 = BootControl::decode(&first[..]).ok();
    let d1 = BootControl::decode(&second[..]).ok();
    let control = match (d0, d1) {
        (Some(x), Some(y)) => {
            if x.sequence >= y.sequence {
                x
            } else {
                y
            }
        }
        (Some(x), None) => x,
        (None, Some(y)) => y,
        (None, None) => return Err(super::invalid(String::from("bootctrl-both-invalid"))),
    };
    let slot_b_before = control.slot_b.tries_remaining;
    let mut current = control;
    for _ in 0..times {
        let (_, next) = current
            .prepare_attempt()
            .map_err(|e| super::invalid(format!("prepare_attempt: {e:?}")))?;
        match next {
            Some(n) => current = n,
            // 选择已非 trial（无可扣减）——按次数语义如实报错，不静默少做。
            None => {
                return Err(super::invalid(String::from(
                    "prepare_attempt: selection is not a trial slot (no tries to consume)",
                )))
            }
        }
    }
    let record = current
        .encode()
        .map_err(|e| super::invalid(format!("encode: {e:?}")))?;
    let mut out = bytes;
    out[base..base + BOOT_CONTROL_RECORD_SIZE].copy_from_slice(&record);
    out[second_off..second_off + BOOT_CONTROL_RECORD_SIZE].copy_from_slice(&record);
    fs::write(&path, &out).map_err(|e| super::invalid(format!("write {}: {e}", path.display())))?;
    println!(
        "BOOTCTRL-PREPARE-ATTEMPT times={times} seq={}->{} slot_b_tries={}->{}",
        control.sequence, current.sequence, slot_b_before, current.slot_b.tries_remaining
    );
    Ok(())
}
