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

#include "sel4_shim.h"

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
    return seL4_Untyped_Retype(service, dest_type, size_bits,
                               dest_root, dest_index, dest_depth,
                               offset, dest_count);
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

seL4_MessageInfo_t eneros_sel4_call(seL4_CPtr dest, seL4_MessageInfo_t msg_info)
{
    return seL4_Call(dest, msg_info);
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

seL4_Error eneros_sel4_arm_page_table_map(seL4_CPtr pt,
                                          seL4_CPtr vspace,
                                          seL4_Word vaddr,
                                          seL4_Word attr)
{
    return seL4_ARM_PageTable_Map(pt, vspace, vaddr, attr);
}

seL4_Word eneros_sel4_arm_default_attributes(void)
{
    return seL4_ARM_Default_VMAttributes;
}

/* ================================================================== */
/* G-KERNEL K1.2：真实 spawn 缺口（ADR-0034）。                        */
/*   TCB_SetSpace / TCB_SetIPCBuffer / ASIDPool_Assign / Page_Unmap   */
/*   与 root-task 固定 cap 常量 getter。                              */
/* ================================================================== */

/* seL4_TCB_SetSpace：把 CSpace / VSpace / IPC buffer 绑到 TCB。      */
seL4_Error eneros_sel4_tcb_set_space(seL4_CPtr tcb,
                                     seL4_CPtr cspace_root,
                                     seL4_Word cspace_root_data,
                                     seL4_CPtr vspace_root,
                                     seL4_Word vspace_root_data,
                                     seL4_CPtr buffer,
                                     seL4_Word buffer_data)
{
    return seL4_TCB_SetSpace(tcb, cspace_root, cspace_root_data,
                             vspace_root, vspace_root_data,
                             buffer, buffer_data);
}

/* seL4_TCB_SetIPCBuffer：设置线程的 IPC buffer（帧 cap + 虚拟地址）。 */
seL4_Error eneros_sel4_tcb_set_ipcbuffer(seL4_CPtr tcb,
                                         seL4_CPtr buffer,
                                         seL4_Word buffer_data)
{
    return seL4_TCB_SetIPCBuffer(tcb, buffer, buffer_data);
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
