//! 诊断打印钩子（39 QEMU 排障临时设施；root 装配时注入，默认无输出）。

use core::sync::atomic::{AtomicUsize, Ordering};

type DiagFn = fn(&str, &str);

static DIAG_HOOK: AtomicUsize = AtomicUsize::new(0);

/// 注册诊断打印函数（root-task 装配时调用一次；0 = 无输出）。
pub fn set_hook(f: fn(&str, &str)) {
    DIAG_HOOK.store(f as usize, Ordering::Relaxed);
}

/// 打印诊断行（无 hook 时 no-op）。
pub fn print(tag: &str, msg: &str) {
    let ptr = DIAG_HOOK.load(Ordering::Relaxed);
    if ptr != 0 {
        // SAFETY: hook 由 set_hook 注册（函数指针），root 单线程装配期。
        let f: DiagFn = unsafe { core::mem::transmute(ptr) };
        f(tag, msg);
    }
}
