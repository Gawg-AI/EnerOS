/*
 * EnerOS seL4 shim implementation (G2 closure / ADR-0023).
 *
 * See sel4_shim.h for the design rationale. Each function here is a
 * non-inline wrapper around a libsel4 `static inline` so that Rust's
 * `extern "C"` FFI has a real symbol to link against.
 *
 * Compiled by build.rs with:
 *   -ffreestanding -fno-builtin -O2 -nostdlib -fno-pic -fno-pie
 *   -I<sel4-src>/libsel4/include
 *   -I<sel4-src>/libsel4/arch_include/arm
 *   -I<sel4-src>/libsel4/sel4_arch_include/aarch64
 *   -I<sel4-src>/libsel4/sel4_plat_include/qemu-arm-virt
 *   -I<sel4-src>/libsel4/mode_include/64
 *   -I<build>/kernel/gen_headers
 *   -I<build>/kernel/gen_config
 *   -I<build>/kernel/autoconf
 *   -I<build>/libsel4/gen_config
 *   -I<build>/libsel4/autoconf
 *   -I<build>/libsel4/include
 *
 * API conformance verified against seL4 14.0.0 generated headers; see
 * sel4_shim.h for the per-API notes.
 *
 * Platform: aarch64 / qemu-arm-virt / seL4 14.0.0.
 */

#include <stdint.h>
#include "sel4_shim.h"

/* DBG 调试打印前向声明（arm_page_map / retype 错误路径使用，定义在文件后部）。 */
static void dbg_print(const char *s);
static void dbg_hex(seL4_Word v);

/* ================================================================== */
/* Minimal freestanding libc stubs（no_std 链接需求）。                */
/* ================================================================== */
/* libsel4 的 __emutls（TLS emulation）与断言在无 libc 环境下需要这些
 * 符号；与 third_party/sel4-root/root_task.c 的最小 stub 同模式。     */
void *malloc(unsigned long size) {
    static unsigned char heap[16384];
    static unsigned long offset = 0;
    unsigned long aligned = (offset + 7u) & ~7u;
    if (aligned + size > sizeof(heap)) {
        return 0;
    }
    offset = aligned + size;
    return heap + aligned;
}

void *memset(void *s, int c, unsigned long n) {
    unsigned char *p = s;
    while (n-- > 0) {
        *p++ = (unsigned char)c;
    }
    return s;
}

void *memcpy(void *dst, const void *src, unsigned long n) {
    unsigned char *d = dst;
    const unsigned char *s = src;
    while (n-- > 0) {
        *d++ = *s++;
    }
    return dst;
}

void abort(void) {
    for (;;) {
        __asm__ volatile("wfe");
    }
}

void __assert_fail(const char *assertion, const char *file, int line,
                   const char *function) {
    (void)assertion;
    (void)file;
    (void)line;
    (void)function;
    abort();
}

/* ================================================================== */
/* Platform-constant getters.                                          */
/* ================================================================== */
seL4_Word eneros_sel4_obj_untyped(void)        { return seL4_UntypedObject; }
seL4_Word eneros_sel4_obj_tcb(void)            { return seL4_TCBObject; }
seL4_Word eneros_sel4_obj_endpoint(void)       { return seL4_EndpointObject; }
seL4_Word eneros_sel4_obj_notification(void)   { return seL4_NotificationObject; }
seL4_Word eneros_sel4_obj_captable(void)       { return seL4_CapTableObject; }
/* seL4 aarch64 has no `seL4_ARM_FrameObject`; the 4 KiB user page used to
 * map an EL0 process image is `seL4_ARM_SmallPageObject`. */
seL4_Word eneros_sel4_obj_arm_frame(void)      { return seL4_ARM_SmallPageObject; }
seL4_Word eneros_sel4_obj_arm_pt(void)         { return seL4_ARM_PageTableObject; }
seL4_Word eneros_sel4_obj_arm_vspace(void)     { return seL4_ARM_VSpaceObject; }

/* seL4_NoRights / seL4_CanRead / ... are macros expanding to
 * `seL4_CapRights_new(...)`, which returns a `seL4_CapRights_t` struct
 * `{ seL4_Uint64 words[1]; }`. Return the raw word so Rust can treat the
 * result as a plain `u64` (the struct is one word, ABI-identical on aarch64). */
seL4_Word eneros_sel4_rights_none(void)   { return seL4_NoRights.words[0]; }
seL4_Word eneros_sel4_rights_read(void)   { return seL4_CanRead.words[0]; }
seL4_Word eneros_sel4_rights_write(void)  { return seL4_CanWrite.words[0]; }
seL4_Word eneros_sel4_rights_grant(void)  { return seL4_CanGrant.words[0]; }
seL4_Word eneros_sel4_rights_all(void)    { return seL4_AllRights.words[0]; }

/* ================================================================== */
/* Debug serial output.                                                */
/* ================================================================== */
seL4_Word eneros_sel4_debug_put_char(char c)
{
    seL4_DebugPutChar(c);
    return 0;
}

/* ================================================================== */
/* Boot info.                                                          */
/* ================================================================== */
seL4_Word eneros_sel4_get_boot_info(void)
{
    return (seL4_Word)seL4_GetBootInfo();
}

/* root-task 启动初始化（BOOT-008）：root-task 用 Rust 汇编 _start（不经过
 * libsel4 的 sel4_start），内核在 capRegister（x0）传入 bootinfo 指针；
 * 这里把 bootinfo 注册进 libsel4（seL4_InitBootInfo），并把 IPC buffer 的
 * 虚拟地址写入 libsel4 的 TLS 变量（__sel4_ipc_buffer），使
 * seL4_Send/Recv/SetMR/GetMR 等 libsel4 API 可用（21-A 遗留缺陷：绕过
 * sel4_start 后 bootinfo=NULL、__sel4_ipc_buffer=NULL）。 */
extern __thread seL4_IPCBuffer *__sel4_ipc_buffer;
void eneros_sel4_boot_init(seL4_Word bi)
{
    seL4_BootInfo *info = (seL4_BootInfo *)bi;
    seL4_InitBootInfo(info);
    __sel4_ipc_buffer = (seL4_IPCBuffer *)(uintptr_t)info->ipcBuffer;
}

/* 设置当前线程的 IPC buffer 指针（__sel4_ipc_buffer，libsel4 TLS）。
 *
 * 被 spawn 的 EL0 服务不是 rootserver：没有 bootinfo（boot_init 不可用），
 * 但 root-task 已通过 seL4_TCB_SetIPCBuffer 为服务 TCB 绑定 buffer 帧并
 * 把 buffer 虚拟地址经 X1（_start 第二参数）传入。服务入口必须调用本函数
 * 把 __sel4_ipc_buffer 指向该地址，否则 ipc_recv/get_mr 等 libsel4 API
 * 在 NULL buffer 上解引用（K2.3 遗留缺陷：svc_ipc_loop 首轮 recv 卡死）。 */
void eneros_sel4_set_ipc_buffer(seL4_Word vaddr)
{
    __sel4_ipc_buffer = (seL4_IPCBuffer *)(uintptr_t)vaddr;
}

/* ================================================================== */
/* Untyped retype.                                                     */
/* seL4 14.0.0: seL4_Untyped_Retype(_service, type, size_bits, root,   */
/*   node_index, node_depth, node_offset, num_objects) — 8 args.       */
/* ================================================================== */
seL4_Error eneros_sel4_untyped_retype(seL4_CPtr service,
                                      seL4_ObjectType dest_type,
                                      seL4_Word size_bits,
                                      seL4_Word dest_count,
                                      seL4_CPtr dest_root,
                                      seL4_Word dest_index,
                                      seL4_Word dest_depth,
                                      seL4_Word offset)
{
    seL4_Error err = seL4_Untyped_Retype(service, dest_type, size_bits,
                                         dest_root, dest_index, dest_depth,
                                         offset, dest_count);
    if (err != 0) {
        dbg_print("DBG-RETYPE-ERR err="); dbg_hex(err);
        dbg_print(" svc="); dbg_hex(service);
        dbg_print(" type="); dbg_hex(dest_type);
        dbg_print(" sz="); dbg_hex(size_bits);
        dbg_print(" idx="); dbg_hex(dest_index);
        dbg_print(" off="); dbg_hex(offset);
        dbg_print("\n");
    }
    return err;
}

/* ================================================================== */
/* CNode operations.                                                   */
/* ================================================================== */
seL4_Error eneros_sel4_cnode_copy(seL4_CPtr dest_root,
                                  seL4_Word dest_index,
                                  seL4_Word dest_depth,
                                  seL4_CPtr src_root,
                                  seL4_Word src_index,
                                  seL4_Word src_depth,
                                  seL4_CapRights_t rights)
{
    return seL4_CNode_Copy(dest_root, dest_index, dest_depth,
                           src_root, src_index, src_depth, rights);
}

seL4_Error eneros_sel4_cnode_mint(seL4_CPtr dest_root,
                                  seL4_Word dest_index,
                                  seL4_Word dest_depth,
                                  seL4_CPtr src_root,
                                  seL4_Word src_index,
                                  seL4_Word src_depth,
                                  seL4_CapRights_t rights,
                                  seL4_Word badge)
{
    return seL4_CNode_Mint(dest_root, dest_index, dest_depth,
                           src_root, src_index, src_depth, rights, badge);
}

seL4_Error eneros_sel4_cnode_move(seL4_CPtr dest_root,
                                  seL4_Word dest_index,
                                  seL4_Word dest_depth,
                                  seL4_CPtr src_root,
                                  seL4_Word src_index,
                                  seL4_Word src_depth)
{
    return seL4_CNode_Move(dest_root, dest_index, dest_depth,
                           src_root, src_index, src_depth);
}

seL4_Error eneros_sel4_cnode_delete(seL4_CPtr root,
                                    seL4_Word index,
                                    seL4_Word depth)
{
    return seL4_CNode_Delete(root, index, depth);
}

seL4_Error eneros_sel4_cnode_revoke(seL4_CPtr root,
                                    seL4_Word index,
                                    seL4_Word depth)
{
    return seL4_CNode_Revoke(root, index, depth);
}

/* ================================================================== */
/* IPC.                                                                */
/* ================================================================== */
void eneros_sel4_set_mr(seL4_Word i, seL4_Word mr)
{
    seL4_SetMR(i, mr);
}

seL4_Word eneros_sel4_get_mr(seL4_Word i)
{
    return seL4_GetMR(i);
}

void eneros_sel4_send(seL4_CPtr dest, seL4_MessageInfo_t msg_info)
{
    seL4_Send(dest, msg_info);
}

seL4_MessageInfo_t eneros_sel4_recv(seL4_CPtr src, seL4_Word *sender)
{
    return seL4_Recv(src, sender);
}

/* 22 §2.W1.3：非阻塞 recv（seL4_Poll：无消息立即返回，errno=seL4_NoError 且
 * msg=0 表示无消息）。supervisor 轮询服务 fault 用，不阻塞 hello 心跳。 */
seL4_MessageInfo_t eneros_sel4_poll(seL4_CPtr src, seL4_Word *sender)
{
    return seL4_Poll(src, sender);
}

seL4_MessageInfo_t eneros_sel4_call(seL4_CPtr dest, seL4_MessageInfo_t msg_info)
{
    return seL4_Call(dest, msg_info);
}

/* 让出 CPU（seL4_Yield）：root-task 阻塞前主动让出，服务有机会运行。 */
void eneros_sel4_yield(void)
{
    seL4_Yield();
}

seL4_MessageInfo_t eneros_sel4_reply_recv(seL4_CPtr dest, seL4_MessageInfo_t msg_info, seL4_Word *sender)
{
    return seL4_ReplyRecv(dest, msg_info, sender);
}

seL4_MessageInfo_t eneros_sel4_message_info_new(seL4_Word label,
                                                seL4_Word caps_unwrapped,
                                                seL4_Word extra_caps,
                                                seL4_Word length)
{
    return seL4_MessageInfo_new(label, caps_unwrapped, extra_caps, length);
}

/* ================================================================== */
/* TCB operations.                                                     */
/* ================================================================== */
seL4_Error eneros_sel4_tcb_write_registers(seL4_CPtr tcb,
                                           seL4_Bool resume_target,
                                           seL4_Word flags,
                                           seL4_Word count,
                                           const seL4_UserContext *regs)
{
    return seL4_TCB_WriteRegisters(tcb, resume_target, flags, count,
                                   (seL4_UserContext *)regs);
}

seL4_Error eneros_sel4_tcb_read_registers(seL4_CPtr tcb,
                                          seL4_Bool suspend_target,
                                          seL4_Word flags,
                                          seL4_Word count,
                                          seL4_UserContext *regs)
{
    return seL4_TCB_ReadRegisters(tcb, suspend_target, flags, count, regs);
}

seL4_Error eneros_sel4_tcb_resume(seL4_CPtr tcb)
{
    return seL4_TCB_Resume(tcb);
}

seL4_Error eneros_sel4_tcb_suspend(seL4_CPtr tcb)
{
    return seL4_TCB_Suspend(tcb);
}

/* seL4_TCB_SetAffinity is declared by libsel4 only when the kernel was
 * built with CONFIG_ENABLE_SMP_SUPPORT (see sel4_client.h guard:
 * `#if (!defined(CONFIG_KERNEL_MCS) && defined(CONFIG_ENABLE_SMP_SUPPORT))`).
 * The current qemu-arm-virt baseline builds SMP-disabled, so the inline is
 * absent. To keep the wrapper linkable in both configs, we call the real
 * API when SMP is enabled and return seL4_IllegalOperation otherwise. */
#ifdef CONFIG_ENABLE_SMP_SUPPORT
seL4_Error eneros_sel4_tcb_set_affinity(seL4_CPtr tcb, seL4_Word affinity)
{
    return seL4_TCB_SetAffinity(tcb, affinity);
}
#else
seL4_Error eneros_sel4_tcb_set_affinity(seL4_CPtr tcb, seL4_Word affinity)
{
    (void)tcb;
    (void)affinity;
    return seL4_IllegalOperation; /* SMP disabled in this kernel build */
}
#endif

seL4_Error eneros_sel4_tcb_set_priority(seL4_CPtr tcb, seL4_CPtr authority, seL4_Word priority)
{
    return seL4_TCB_SetPriority(tcb, authority, priority);
}

seL4_Word eneros_sel4_user_context_n_registers(void)
{
    return sizeof(seL4_UserContext) / sizeof(seL4_Word);
}

/* ================================================================== */
/* VSpace / page mapping.                                              */
/* ================================================================== */
seL4_Error eneros_sel4_arm_page_map(seL4_CPtr page,
                                    seL4_CPtr vspace,
                                    seL4_Word vaddr,
                                    seL4_CapRights_t rights,
                                    seL4_Word attr)
{
    return seL4_ARM_Page_Map(page, vspace, vaddr, rights, attr);
}

/* DBG-TEMP: 经 seL4_DebugPutChar（内核 syscall）的调试打印（用后还原）。 */
static void dbg_putc(char c) {
    seL4_DebugPutChar((seL4_Uint8)c);
}
static void dbg_print(const char *s) {
    while (*s) { dbg_putc(*s++); }
}
static void dbg_hex(seL4_Word v) {
    const char *h = "0123456789abcdef";
    char buf[17]; buf[16] = 0;
    for (int i = 15; i >= 0; i--) { buf[i] = h[v & 0xf]; v >>= 4; }
    dbg_print(buf);
}

seL4_Word eneros_sel4_debug_cap_identify(seL4_CPtr cap)
{
    return seL4_DebugCapIdentify(cap);
}

seL4_Error eneros_sel4_arm_page_table_map(seL4_CPtr pt,
                                          seL4_CPtr vspace,
                                          seL4_Word vaddr,
                                          seL4_Word attr)
{
    seL4_Error err = seL4_ARM_PageTable_Map(pt, vspace, vaddr, attr);
    if (err != 0) {
        dbg_print("DBG-PTMAP err="); dbg_hex(err);
        dbg_print(" pt="); dbg_hex(pt);
        dbg_print(" vs="); dbg_hex(vspace);
        dbg_print(" va="); dbg_hex(vaddr);
        dbg_print(" mr0="); dbg_hex(seL4_GetMR(0));
        dbg_print(" mr1="); dbg_hex(seL4_GetMR(1));
        dbg_print("\n");
    }
    return err;
}

seL4_Word eneros_sel4_arm_default_attributes(void)
{
    return seL4_ARM_Default_VMAttributes;
}

/* K2.3：seL4_TCB_BindNotification（服务故障通知，21-A K2.3 隔离验证）。 */
seL4_Error eneros_sel4_tcb_bind_notification(seL4_CPtr tcb, seL4_CPtr ntfn)
{
    return seL4_TCB_BindNotification(tcb, ntfn);
}

/* ================================================================== */
/* G-KERNEL K1.2：真实 spawn 缺口（ADR-0034）。                        */
/*   TCB_SetSpace / TCB_SetIPCBuffer / ASIDPool_Assign / Page_Unmap   */
/*   与 root-task 固定 cap 常量 getter。                              */
/* ================================================================== */

/* seL4_TCB_SetSpace：把 CSpace / VSpace 绑到 TCB（seL4 14.0.0 API：
 * fault_ep 为独立 word 参数；IPC buffer 经 seL4_TCB_SetIPCBuffer 单独绑定）。
 * 非 MCS 配置下 fault_ep 仅作 word 记录（内核不校验其有效性）。         */
seL4_Error eneros_sel4_tcb_set_space(seL4_CPtr tcb,
                                     seL4_Word fault_ep,
                                     seL4_CPtr cspace_root,
                                     seL4_Word cspace_root_data,
                                     seL4_CPtr vspace_root,
                                     seL4_Word vspace_root_data)
{
    return seL4_TCB_SetSpace(tcb, fault_ep, cspace_root, cspace_root_data,
                             vspace_root, vspace_root_data);
}

/* seL4_TCB_SetIPCBuffer：设置线程的 IPC buffer（帧 cap + 虚拟地址）。
 * 注意：libsel4 生成的签名是 `(_service, buffer_addr, bufferFrame_cap)`——
 * 第二个参数是 buffer 的虚拟地址（MR0），第三个才是帧 cap（SetCap 0）。
 * 本 shim 的参数名沿用 Rust safe 封装语义（buffer=帧 cap、buffer_data=地址），
 * 因此必须交换顺序再传给 seL4（21-A 遗留缺陷：参数错位导致
 * `Lookup of extra caps failed`，QEMU 实测 fault 地址 = buffer 虚拟地址）。 */
seL4_Error eneros_sel4_tcb_set_ipcbuffer(seL4_CPtr tcb,
                                         seL4_CPtr buffer,
                                         seL4_Word buffer_data)
{
    return seL4_TCB_SetIPCBuffer(tcb, buffer_data, buffer);
}

/* seL4_ARM_ASIDPool_Assign：从 ASID pool 给 VSpace 分配 ASID。       */
/* aarch64 上 VSpace 未绑 ASID 时页映射走 findVSpaceForASID 会失败。  */
seL4_Error eneros_sel4_arm_asid_pool_assign(seL4_CPtr pool,
                                            seL4_CPtr vspace)
{
    return seL4_ARM_ASIDPool_Assign(pool, vspace);
}

/* seL4_ARM_Page_Unmap：解除 frame 的映射（frame 只能映射一个地址，   */
/* 重新映射前必须 unmap，见 ADR-0034）。                               */
seL4_Error eneros_sel4_arm_page_unmap(seL4_CPtr page)
{
    return seL4_ARM_Page_Unmap(page);
}

/* root-task 固定 cap 常量（bootinfo.h 编译期宏；经 getter 暴露避免   */
/* Rust 侧硬编码漂移）。                                              */
seL4_Word eneros_sel4_cap_init_thread_tcb(void)
{
    return seL4_CapInitThreadTCB;
}

seL4_Word eneros_sel4_cap_init_thread_cnode(void)
{
    return seL4_CapInitThreadCNode;
}

seL4_Word eneros_sel4_cap_init_thread_vspace(void)
{
    return seL4_CapInitThreadVSpace;
}

seL4_Word eneros_sel4_cap_init_thread_asid_pool(void)
{
    return seL4_CapInitThreadASIDPool;
}

/* ============================================================
 * WP-26.8（26-C §2.2）：IRQ 子系统绑定（seL4 14 API）
 * ============================================================ */

/* 从 IRQControl cap 获取 IRQHandler cap（Get）→ 目标槽。
 * seL4_IRQControl_Get(irq_ctrl, irq, dest_root, dest_index, dest_depth)。 */
seL4_Error eneros_sel4_irq_control_get(seL4_CPtr irq_ctrl, seL4_Word irq,
                                       seL4_CPtr dest_root, seL4_Word dest_index,
                                       seL4_Word dest_depth)
{
    return seL4_IRQControl_Get(irq_ctrl, irq, dest_root, dest_index, dest_depth);
}

/* IRQHandler cap 绑定 notification：中断触发 → ntfn 置位（badge 由 mint 设定）。 */
seL4_Error eneros_sel4_irq_handler_set_notification(seL4_CPtr handler, seL4_CPtr ntfn)
{
    return seL4_IRQHandler_SetNotification(handler, ntfn);
}

/* 中断处理完成后 Ack（必须 Ack，否则中断不重触发）。 */
seL4_Error eneros_sel4_irq_handler_ack(seL4_CPtr handler)
{
    return seL4_IRQHandler_Ack(handler);
}

/* 解绑中断（handler cap 生命周期结束）。 */
seL4_Error eneros_sel4_irq_handler_clear(seL4_CPtr handler)
{
    return seL4_IRQHandler_Clear(handler);
}

/* 初始 IRQControl cap 槽号（seL4_CapIRQControl；Rust 侧不硬编码）。 */
seL4_Word eneros_sel4_cap_irq_control(void)
{
    return seL4_CapIRQControl;
}
