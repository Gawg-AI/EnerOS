//! SPEC-C：littlefs2 C 侧字符串原语的最小 no_std 补齐。
//!
//! 背景：`eneros-fs` 此前无 target 消费者，本卷 svc-telemetry 首次把
//! littlefs2（c-stubs 构建）链入 aarch64-none-elf 产物时暴露三个未定义
//! 符号（strspn/strcspn/strchr；newlib 其余符号已由编译器内置/驱动面
//! 提供）。此 shim 与 littlefs2 官方 c-stubs 同语义：仅服务 LFS 内部
//! 路径解析，不做 locale/多字节处理。

/// `strchr(s, c)`：返回首个等于 c 的字节指针；c==0 时返回指向终止符的
/// 指针；未命中 → null。语义按 C11 7.24.5.2。
#[no_mangle]
pub unsafe extern "C" fn strchr(s: *const u8, c: core::ffi::c_int) -> *mut u8 {
    let target = c as u8;
    let mut p = s;
    loop {
        let b = *p;
        if b == target {
            return p.cast_mut();
        }
        if b == 0 {
            return core::ptr::null_mut();
        }
        p = p.add(1);
    }
}

/// `strspn(s, accept)`：s 前缀中完全由 accept 字符集构成的最大长度。
#[no_mangle]
pub unsafe extern "C" fn strspn(s: *const u8, accept: *const u8) -> usize {
    let mut n = 0usize;
    let mut p = s;
    while *p != 0 && contains(accept, *p) {
        n += 1;
        p = p.add(1);
    }
    n
}

/// `strcspn(s, reject)`：s 前缀中不含任何 reject 字符的最大长度。
#[no_mangle]
pub unsafe extern "C" fn strcspn(s: *const u8, reject: *const u8) -> usize {
    let mut n = 0usize;
    let mut p = s;
    while *p != 0 && !contains(reject, *p) {
        n += 1;
        p = p.add(1);
    }
    n
}

/// 在以 NUL 结尾的集合串中查找字节（内部助手）。
unsafe fn contains(set: *const u8, needle: u8) -> bool {
    let mut q = set;
    loop {
        let b = *q;
        if b == 0 {
            return false;
        }
        if b == needle {
            return true;
        }
        q = q.add(1);
    }
}
