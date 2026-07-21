//! EnerOS 电能表校准 crate — 校准系数、精度等级与校准流程（v0.51.1）.
//!
//! 本 crate 为电能计量驱动层提供校准能力，覆盖：
//! - [`CalibCoeffs`] — CT/PT 变比、相位校正、电压/电流偏置等线性校正参数
//! - [`AccuracyClass`] — 0.2S / 0.5S / 1.0 / 2.0 四级精度等级
//! - [`MeterReading`] / [`CalibResult`] — 读数与校准结果
//! - [`MeterCalibration`] trait — 校准抽象（系数应用 / 误差测量 / 精度分级）
//! - [`CalibStore`] trait + [`InMemoryCalibStore`] — 校准系数持久化抽象
//! - [`calibrate_meter`] / [`verify_accuracy`] — 校准与校验入口函数
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，零外部依赖（D10）。
//!
//! # 偏差声明（D8~D10）
//!
//! | 编号 | 偏差内容 | 理由 |
//! |------|---------|------|
//! | D8 | crate 放置于 `crates/drivers/calibration/` | 电能表校准属于驱动层增强（抄表读数的线性校正），归入 drivers 子系统而非 protocols（校准不涉及通信协议，仅做数值映射） |
//! | D9 | `CalibStore` trait 抽象持久化，不直接依赖文件系统 | 解耦校准逻辑与存储后端；`InMemoryCalibStore` 用于测试/启动期，文件系统（littlefs2）实现后置到后续版本 |
//! | D10 | 零外部依赖（pure computation + data structures） | 校准仅涉及 f64 运算与 `BTreeMap`，无需任何外部 crate；保证交叉编译零障碍、SBOM 最小化 |
//!
//! # 模块结构
//!
//! - `coeffs` — 校准系数
//! - `accuracy` — 精度等级
//! - `result` — 读数与校准结果
//! - `trait_def` — MeterCalibration trait（模块名避开 `trait` 关键字）
//! - `store` — 持久化抽象
//! - `func` — 校准/校验函数与默认校准器
//!
//! # 用法
//!
//! ```ignore
//! use eneros_calibration::{calibrate_meter, AccuracyClass, MeterReading};
//!
//! let measured = MeterReading { voltage: 200.0, current: 9.5, power: 1900.0, energy: 1900.0 };
//! let reference = MeterReading { voltage: 220.0, current: 10.0, power: 2200.0, energy: 2200.0 };
//! let result = calibrate_meter(1.0, 1.0, &measured, &reference, AccuracyClass::Class0_5S, 1000);
//! assert!(result.passed);
//! ```

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod accuracy;
pub mod coeffs;
pub mod func;
pub mod result;
pub mod store;
pub mod trait_def;

pub use accuracy::{is_within_class, AccuracyClass};
pub use coeffs::CalibCoeffs;
pub use func::{calibrate_meter, verify_accuracy, DefaultCalibrator};
pub use result::{CalibResult, MeterReading};
pub use store::{CalibStore, InMemoryCalibStore};
pub use trait_def::MeterCalibration;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::func::DefaultCalibrator;
    use crate::trait_def::MeterCalibration;

    #[test]
    fn test_calib_coeffs_default() {
        let c = CalibCoeffs::default();
        assert_eq!(c.ct_ratio, 1.0);
        assert_eq!(c.pt_ratio, 1.0);
        assert_eq!(c.phase_correction, 0.0);
        assert_eq!(c.offset_voltage, 0.0);
        assert_eq!(c.offset_current, 0.0);
        assert_eq!(c.calibrated_at, 0);
    }

    #[test]
    fn test_apply_voltage_current() {
        let c = CalibCoeffs {
            ct_ratio: 200.0,
            pt_ratio: 100.0,
            phase_correction: 0.0,
            offset_voltage: 1.5,
            offset_current: 0.05,
            calibrated_at: 0,
        };
        // raw=0.005 A * 200 + 0.05 = 1.05 A
        assert!((c.apply_current(0.005) - 1.05).abs() < 1e-9);
        // raw=1.0 V * 100 + 1.5 = 101.5 V
        assert!((c.apply_voltage(1.0) - 101.5).abs() < 1e-9);

        // Default（ratio=1.0, offset=0）=> passthrough
        let d = CalibCoeffs::default();
        assert!((d.apply_voltage(220.0) - 220.0).abs() < 1e-9);
        assert!((d.apply_current(5.0) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_accuracy_class_max_error_pct() {
        assert!((AccuracyClass::Class0_2S.max_error_pct() - 0.2).abs() < 1e-9);
        assert!((AccuracyClass::Class0_5S.max_error_pct() - 0.5).abs() < 1e-9);
        assert!((AccuracyClass::Class1_0.max_error_pct() - 1.0).abs() < 1e-9);
        assert!((AccuracyClass::Class2_0.max_error_pct() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn test_is_within_class_boundary() {
        // 0.2S：边界 0.2（含）
        assert!(is_within_class(0.2, AccuracyClass::Class0_2S));
        assert!(is_within_class(-0.2, AccuracyClass::Class0_2S));
        assert!(is_within_class(0.0, AccuracyClass::Class0_2S));
        assert!(!is_within_class(0.2001, AccuracyClass::Class0_2S));

        // 2.0：边界 2.0（含）
        assert!(is_within_class(2.0, AccuracyClass::Class2_0));
        assert!(!is_within_class(2.0001, AccuracyClass::Class2_0));
        assert!(is_within_class(1.5, AccuracyClass::Class2_0));

        // 0.5S 与 1.0 各边界
        assert!(is_within_class(0.5, AccuracyClass::Class0_5S));
        assert!(!is_within_class(0.6, AccuracyClass::Class0_5S));
        assert!(is_within_class(1.0, AccuracyClass::Class1_0));
        assert!(!is_within_class(1.1, AccuracyClass::Class1_0));
    }

    #[test]
    fn test_store_save_load_roundtrip() {
        let mut store = InMemoryCalibStore::new();
        let coeffs = CalibCoeffs {
            ct_ratio: 200.0,
            pt_ratio: 100.0,
            phase_correction: 0.5,
            offset_voltage: 1.0,
            offset_current: 0.02,
            calibrated_at: 123456,
        };
        store.save(42, &coeffs);
        let loaded = store.load(42).expect("should exist after save");
        assert_eq!(loaded, coeffs);
    }

    #[test]
    fn test_store_load_nonexistent_returns_none() {
        let store = InMemoryCalibStore::new();
        assert!(store.load(999).is_none());

        // 保存后存在，另一个 id 仍为 None
        let mut store2 = InMemoryCalibStore::new();
        store2.save(1, &CalibCoeffs::default());
        assert!(store2.load(2).is_none());
    }

    #[test]
    fn test_calibrate_meter_perfect_readings() {
        let measured = MeterReading {
            voltage: 220.0,
            current: 10.0,
            power: 2200.0,
            energy: 2200.0,
        };
        let reference = MeterReading {
            voltage: 220.0,
            current: 10.0,
            power: 2200.0,
            energy: 2200.0,
        };
        let result = calibrate_meter(
            1.0,
            1.0,
            &measured,
            &reference,
            AccuracyClass::Class0_2S,
            1000,
        );
        assert!(result.before_error_pct.abs() < 1e-9);
        assert!(result.after_error_pct.abs() < 1e-6);
        assert!(result.passed);
        assert_eq!(result.measured_at, 1000);
        assert_eq!(result.target_class, AccuracyClass::Class0_2S);
        assert_eq!(result.coeffs.calibrated_at, 1000);
    }

    #[test]
    fn test_calibrate_meter_with_error() {
        // 被校表读数偏低
        let measured = MeterReading {
            voltage: 200.0,
            current: 9.5,
            power: 1900.0,
            energy: 1900.0,
        };
        let reference = MeterReading {
            voltage: 220.0,
            current: 10.0,
            power: 2200.0,
            energy: 2200.0,
        };
        let result = calibrate_meter(
            1.0,
            1.0,
            &measured,
            &reference,
            AccuracyClass::Class0_5S,
            2000,
        );
        // 校准前误差约 -13.6%（负偏）
        assert!(result.before_error_pct < 0.0);
        assert!(result.before_error_pct.abs() > 1.0);
        // 校准后误差应显著减小（偏置已对齐电压/电流，功率由 V*I 重算）
        assert!(result.after_error_pct.abs() < result.before_error_pct.abs());
        // 校准后应通过 0.5S（功率 220*10=2200 == 参考功率 2200）
        assert!(result.passed);
        assert_eq!(result.measured_at, 2000);
        // 偏置应为：220 - 200*1 = 20 V, 10 - 9.5*1 = 0.5 A
        assert!((result.coeffs.offset_voltage - 20.0).abs() < 1e-9);
        assert!((result.coeffs.offset_current - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_verify_accuracy_pass() {
        let measured = MeterReading {
            voltage: 220.0,
            current: 10.0,
            power: 2199.0, // 误差 ~ -0.045%
            energy: 2199.0,
        };
        let reference = MeterReading {
            voltage: 220.0,
            current: 10.0,
            power: 2200.0,
            energy: 2200.0,
        };
        assert!(verify_accuracy(
            &measured,
            &reference,
            AccuracyClass::Class0_2S
        ));
        assert!(verify_accuracy(
            &measured,
            &reference,
            AccuracyClass::Class0_5S
        ));
    }

    #[test]
    fn test_verify_accuracy_fail() {
        let measured = MeterReading {
            voltage: 220.0,
            current: 10.0,
            power: 2300.0, // 误差 ~ +4.5%
            energy: 2300.0,
        };
        let reference = MeterReading {
            voltage: 220.0,
            current: 10.0,
            power: 2200.0,
            energy: 2200.0,
        };
        assert!(!verify_accuracy(
            &measured,
            &reference,
            AccuracyClass::Class0_2S
        ));
        assert!(!verify_accuracy(
            &measured,
            &reference,
            AccuracyClass::Class2_0
        ));
    }

    #[test]
    fn test_default_calibrator_classify_accuracy() {
        let cal = DefaultCalibrator;
        // 误差 0.1% => 最高 0.2S
        assert_eq!(cal.classify_accuracy(0.1), AccuracyClass::Class0_2S);
        assert_eq!(cal.classify_accuracy(-0.1), AccuracyClass::Class0_2S);
        // 误差 0.3% => 0.5S
        assert_eq!(cal.classify_accuracy(0.3), AccuracyClass::Class0_5S);
        // 误差 0.8% => 1.0
        assert_eq!(cal.classify_accuracy(0.8), AccuracyClass::Class1_0);
        // 误差 1.5% => 2.0
        assert_eq!(cal.classify_accuracy(1.5), AccuracyClass::Class2_0);
        // 误差 3.0%（超出所有等级）=> 回退到最宽 2.0
        assert_eq!(cal.classify_accuracy(3.0), AccuracyClass::Class2_0);
    }

    #[test]
    fn test_default_calibrator_apply_coefficients() {
        let cal = DefaultCalibrator;
        let reading = MeterReading {
            voltage: 1.0,
            current: 0.005,
            power: 0.005,
            energy: 100.0,
        };
        let coeffs = CalibCoeffs {
            ct_ratio: 200.0,
            pt_ratio: 100.0,
            phase_correction: 0.0,
            offset_voltage: 1.5,
            offset_current: 0.05,
            calibrated_at: 0,
        };
        let out = cal.apply_coefficients(&reading, &coeffs);
        // voltage = 1.0*100 + 1.5 = 101.5
        assert!((out.voltage - 101.5).abs() < 1e-9);
        // current = 0.005*200 + 0.05 = 1.05
        assert!((out.current - 1.05).abs() < 1e-9);
        // power = 101.5 * 1.05 = 106.575
        assert!((out.power - 106.575).abs() < 1e-9);
        // energy = 100 * 200 * 100 = 2_000_000
        assert!((out.energy - 2_000_000.0).abs() < 1e-6);
    }
}
