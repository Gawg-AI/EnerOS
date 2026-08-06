//! GPIO 位操作纯逻辑（HAL-003 可测性抽取）。
//!
//! 不依赖任何寄存器/asm，host 可直接测试；`arm64::gpio` 引用。

/// HAL-003：GPIO DATA 寄存器读-改-写（只动 `pin` 位）。
///
/// 纯逻辑（无 MMIO），host 可测：验证 set 一个引脚不会清零其他引脚。
#[allow(dead_code)]
pub(crate) fn apply_set_pin(data: u32, pin: u32, val: bool) -> u32 {
    let mask = 1u32 << pin;
    if val {
        data | mask
    } else {
        data & !mask
    }
}

/// HAL-003：GPIO PUD 寄存器 2-bit 位域读-改-写。
///
/// `pin_in_reg` 是引脚在所在 PUD 寄存器内的编号（0..16）；`pull_val` 为
/// 0=none / 1=up / 2=down。只覆盖该引脚的 2-bit 位域，保留同寄存器其他引脚。
#[allow(dead_code)]
pub(crate) fn apply_pull(pud: u32, pin_in_reg: u32, pull_val: u32) -> u32 {
    let shift = pin_in_reg * 2;
    let mask = 0b11u32 << shift;
    (pud & !mask) | ((pull_val & 0b11) << shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_pin_preserves_other_pins() {
        let data = 0b1010_0000u32;
        // 置位 pin 3 → 只加 bit3
        assert_eq!(apply_set_pin(data, 3, true), 0b1010_1000);
        // 清零 pin 7 → 只清 bit7
        assert_eq!(apply_set_pin(data, 7, false), 0b0010_0000);
        // 置位已置位的 pin → 不变
        assert_eq!(apply_set_pin(data, 5, true), data);
    }

    #[test]
    fn set_pin_no_clobber_others_hallmark() {
        // 旧实现 `w32(GPIO_DATA, mask)` 会把 data 中其他 31 位清零；
        // 新实现必须保留（HAL-003 回归）。
        let data = 0xFFFF_FFFFu32;
        assert_eq!(apply_set_pin(data, 0, true), 0xFFFF_FFFF);
        assert_eq!(apply_set_pin(data, 31, false), 0x7FFF_FFFF);
    }

    #[test]
    fn pull_field_preserves_other_pins() {
        // pud = 0b00011011：pin0=11, pin1=10, pin2=01, pin3=00（低位在右）
        let pud = 0b0001_1011u32;
        // pin1（bits 2-3）→ up(01)：0b00010111 = pin0=11, pin1=01, pin2=01, pin3=00
        assert_eq!(apply_pull(pud, 1, 1), 0b0001_0111);
        // pin0（bits 0-1）→ down(10)：0b00011010 = pin0=10, pin1=10, pin2=01, pin3=00
        assert_eq!(apply_pull(pud, 0, 2), 0b0001_1010);
        // pin2（bits 4-5）→ none(00)：只清 pin2，保留 pin0=11/pin1=10 → 0b00001011
        assert_eq!(apply_pull(pud, 2, 0), 0b0000_1011);
        // 相邻 pin（pin3，bits 6-7）→ up(01)：0b01011011 = pin3=01, pin2=01, pin1=10, pin0=11
        assert_eq!(apply_pull(pud, 3, 1), 0b0101_1011);
    }
}
