//! Raw FFI declarations against the C shim (`shim/sel4_shim.c`).
//!
//! Only compiled on the seL4 target (`cfg(sel4_target)`). Host builds use
//! `host.rs` stubs instead. Every function here maps 1:1 to a wrapper in
//! `shim/sel4_shim.h` — never call libsel4 symbols directly from Rust,
//! because most libsel4 API is `static inline` and produces no linkable
//! symbol.

#![allow(non_snake_case)]
#![allow(non_camel_case_types)]

use crate::types::*;

// The C shim uses seL4's own types, which on aarch64 are typedef'd to
// `uint64_t`. Our Rust aliases (`Word`, `CPtr`, `Error`, ...) are all `u64`
// or `i64`, so the ABI matches. We re-use them directly.

// ----------------------------------------------------------------------------
// Platform-constant getters.
// ----------------------------------------------------------------------------
extern "C" {
    pub fn eneros_sel4_obj_untyped() -> Word;
    pub fn eneros_sel4_obj_tcb() -> Word;
    pub fn eneros_sel4_obj_endpoint() -> Word;
    pub fn eneros_sel4_obj_notification() -> Word;
    pub fn eneros_sel4_obj_captable() -> Word;
    pub fn eneros_sel4_obj_arm_frame() -> Word;
    pub fn eneros_sel4_obj_arm_pt() -> Word;
    pub fn eneros_sel4_obj_arm_vspace() -> Word;

    pub fn eneros_sel4_rights_none() -> Word;
    pub fn eneros_sel4_rights_read() -> Word;
    pub fn eneros_sel4_rights_write() -> Word;
    pub fn eneros_sel4_rights_grant() -> Word;
    pub fn eneros_sel4_rights_all() -> Word;

    pub fn eneros_sel4_user_context_n_registers() -> Word;
    pub fn eneros_sel4_arm_default_attributes() -> Word;

    /// 调试：识别 cap 类型（`seL4_DebugCapIdentify`，CONFIG_DEBUG_BUILD）。
    pub fn eneros_sel4_debug_cap_identify(cap: Word) -> Word;

    // ADR-0034（K1.2）：root-task 固定 cap 常量（bootinfo.h 宏，经 shim getter）。
    pub fn eneros_sel4_cap_init_thread_tcb() -> Word;
    pub fn eneros_sel4_cap_init_thread_cnode() -> Word;
    pub fn eneros_sel4_cap_init_thread_vspace() -> Word;
    pub fn eneros_sel4_cap_init_thread_asid_pool() -> Word;
}

// ----------------------------------------------------------------------------
// Debug + bootinfo.
// ----------------------------------------------------------------------------
extern "C" {
    /// `seL4_DebugPutChar(c)` — write one byte to the seL4 debug serial.
    pub fn eneros_sel4_debug_put_char(c: u8) -> Word;

    /// `seL4_GetBootInfo()` — return the capability to the bootinfo page.
    /// The returned word is a `seL4_BootInfo*` cast to `seL4_Word`.
    pub fn eneros_sel4_get_boot_info() -> Word;

    /// root-task 启动初始化（BOOT-008）：注册 bootinfo + IPC buffer TLS。
    pub fn eneros_sel4_boot_init(bi: Word);

    /// 设置当前线程 IPC buffer 指针（`__sel4_ipc_buffer` TLS）。
    /// 被 spawn 的 EL0 服务无 bootinfo，用 root-task 传入的 buffer vaddr
    /// 初始化（K2.3：否则 ipc_recv 在 NULL buffer 上解引用卡死）。
    pub fn eneros_sel4_set_ipc_buffer(vaddr: Word);

    /// 让出 CPU（`seL4_Yield`）：root-task 阻塞前主动让出，服务有机会运行。
    pub fn eneros_sel4_yield();
}

// ----------------------------------------------------------------------------
// Untyped retype.
// ----------------------------------------------------------------------------
extern "C" {
    pub fn eneros_sel4_untyped_retype(
        service: CPtr,
        dest_type: ObjectType,
        size_bits: Word,
        dest_count: Word,
        dest_root: CPtr,
        dest_index: Word,
        dest_depth: Word,
        offset: Word,
    ) -> Error;
}

// ----------------------------------------------------------------------------
// CNode operations.
// ----------------------------------------------------------------------------
extern "C" {
    pub fn eneros_sel4_cnode_copy(
        dest_root: CPtr,
        dest_index: Word,
        dest_depth: Word,
        src_root: CPtr,
        src_index: Word,
        src_depth: Word,
        rights: CapRights,
    ) -> Error;

    pub fn eneros_sel4_cnode_mint(
        dest_root: CPtr,
        dest_index: Word,
        dest_depth: Word,
        src_root: CPtr,
        src_index: Word,
        src_depth: Word,
        rights: CapRights,
        badge: Word,
    ) -> Error;

    pub fn eneros_sel4_cnode_move(
        dest_root: CPtr,
        dest_index: Word,
        dest_depth: Word,
        src_root: CPtr,
        src_index: Word,
        src_depth: Word,
    ) -> Error;

    pub fn eneros_sel4_cnode_delete(root: CPtr, index: Word, depth: Word) -> Error;
    pub fn eneros_sel4_cnode_revoke(root: CPtr, index: Word, depth: Word) -> Error;
}

// ----------------------------------------------------------------------------
// IPC.
// ----------------------------------------------------------------------------
extern "C" {
    pub fn eneros_sel4_set_mr(i: Word, mr: Word);
    pub fn eneros_sel4_get_mr(i: Word) -> Word;

    /// `seL4_Send` — send-only (no reply); returns nothing.
    pub fn eneros_sel4_send(dest: CPtr, msg_info: MessageInfo);
    pub fn eneros_sel4_recv(src: CPtr, sender: *mut Word) -> MessageInfo;
    pub fn eneros_sel4_poll(src: CPtr, sender: *mut Word) -> MessageInfo;
    pub fn eneros_sel4_call(dest: CPtr, msg_info: MessageInfo) -> MessageInfo;
    pub fn eneros_sel4_reply_recv(
        dest: CPtr,
        msg_info: MessageInfo,
        sender: *mut Word,
    ) -> MessageInfo;

    pub fn eneros_sel4_message_info_new(
        label: Word,
        caps_unwrapped: Word,
        extra_caps: Word,
        length: Word,
    ) -> MessageInfo;
}

// ----------------------------------------------------------------------------
// TCB operations.
// ----------------------------------------------------------------------------
extern "C" {
    pub fn eneros_sel4_tcb_write_registers(
        tcb: CPtr,
        resume_target: Bool,
        flags: Word,
        count: Word,
        regs: *const UserContext,
    ) -> Error;

    pub fn eneros_sel4_tcb_read_registers(
        tcb: CPtr,
        suspend_target: Bool,
        flags: Word,
        count: Word,
        regs: *mut UserContext,
    ) -> Error;

    pub fn eneros_sel4_tcb_resume(tcb: CPtr) -> Error;
    pub fn eneros_sel4_tcb_suspend(tcb: CPtr) -> Error;
    pub fn eneros_sel4_tcb_set_affinity(tcb: CPtr, affinity: Word) -> Error;
    pub fn eneros_sel4_tcb_set_priority(tcb: CPtr, authority: CPtr, priority: Word) -> Error;

    // ADR-0034（K1.2）：TCB 绑定 CSpace/VSpace（真实 spawn 必需；seL4 14.0.0
    // API 含 fault_ep word 参数，IPC buffer 由 tcb_set_ipcbuffer 单独绑定）。
    pub fn eneros_sel4_tcb_set_space(
        tcb: CPtr,
        fault_ep: Word,
        cspace_root: CPtr,
        cspace_root_data: Word,
        vspace_root: CPtr,
        vspace_root_data: Word,
    ) -> Error;

    pub fn eneros_sel4_tcb_set_ipcbuffer(tcb: CPtr, buffer: CPtr, buffer_data: Word) -> Error;

    // ADR-0034/K2.3：把 notification 绑到 TCB（服务故障通知）。
    pub fn eneros_sel4_tcb_bind_notification(tcb: CPtr, ntfn: CPtr) -> Error;
}

// ----------------------------------------------------------------------------
// ARM VSpace / page mapping.
// ----------------------------------------------------------------------------
extern "C" {
    pub fn eneros_sel4_arm_page_map(
        page: CPtr,
        vspace: CPtr,
        vaddr: Word,
        rights: CapRights,
        attr: ArmVmAttributes,
    ) -> Error;

    pub fn eneros_sel4_arm_page_table_map(
        pt: CPtr,
        vspace: CPtr,
        vaddr: Word,
        attr: ArmVmAttributes,
    ) -> Error;

    // ADR-0034（K1.2）：aarch64 VSpace 必须绑 ASID；frame 只能映射一个地址，
    // 重新映射前必须 unmap（K1.3 方案 A 临时窗口拷贝依赖）。
    pub fn eneros_sel4_arm_asid_pool_assign(pool: CPtr, vspace: CPtr) -> Error;
    pub fn eneros_sel4_arm_page_unmap(page: CPtr) -> Error;
}
