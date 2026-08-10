/*
 * EnerOS seL4 shim header (G2 closure / ADR-0023).
 *
 * libsel4 declares most of its API as `static inline` functions in headers
 * (the compiled `libsel4.a` archive is only ~1.8 KiB and contains just a
 * handful of non-inline symbols). A Rust `extern "C"` cannot call a C
 * `static inline` directly, so this shim provides non-inline C wrappers —
 * one real symbol per seL4 API the Rust userland needs — that the Rust
 * FFI links against.
 *
 * The shim is compiled by `build.rs` with the libsel4 + generated config
 * include paths that `third_party/sel4-build.sh` produces, then archived
 * into `libeneros_sel4_shim.a` and linked ahead of `libsel4.a` / `libgcc.a`.
 *
 * This is exactly the "受版本控制的 C shim、header、build.rs、静态 archive
 * 和链接顺序" mandated by Book 5 §v0.289.0 (L296) and MASTER G2.
 *
 * Platform: aarch64 / qemu-arm-virt / seL4 14.0.0 (ADR-0023 pinned baseline).
 *
 * API conformance: verified against seL4 14.0.0 libsel4 generated headers
 * (`shared_types_gen.h`, `interfaces/sel4_client.h`, arch `objecttype.h`):
 *   - seL4_MessageInfo_t / seL4_CapRights_t are the struct typedefs (single
 *     `seL4_Uint64 words[1]`); on aarch64 AAPCS64 a one-word struct is
 *     returned/passed in a register, so it is ABI-identical to `seL4_Word`.
 *     The Rust side therefore types these as `u64` (`Word`).
 *   - seL4_Untyped_Retype takes 8 args including `size_bits`.
 *   - seL4_ARM_FrameObject does not exist; the 4 KiB page object is
 *     `seL4_ARM_SmallPageObject`.
 *   - seL4_TCB_SetAffinity is gated on CONFIG_ENABLE_SMP_SUPPORT.
 */
#ifndef ENEROS_SEL4_SHIM_H
#define ENEROS_SEL4_SHIM_H

/* sel4.h is the umbrella header: it pulls in types.h, objecttype.h, errors.h,
 * functions.h, syscalls.h, faults.h, bootinfo.h, and the generated sel4_arch
 * types_gen.h (seL4_MessageInfo_t, seL4_UserContext) + arch headers. There is
 * no `sel4/messages.h` in libsel4 (verified against seL4 14.0.0 source); all
 * IPC message-register / send-recv / MessageInfo APIs used below are declared
 * as `static inline`s in sel4.h itself, which this shim wraps as real symbols. */
#include <sel4/sel4.h>
#include <sel4/types.h>
#include <sel4/objecttype.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ */
/* Platform-constant getters.                                          */
/* seL4 object type enum values are platform/config-specific; instead  */
/* of hardcoding them in Rust (fragile across seL4 versions), the C    */
/* compiler resolves them from the generated headers and exports the   */
/* real value via these getters.                                       */
/* ------------------------------------------------------------------ */
seL4_Word eneros_sel4_obj_untyped(void);
seL4_Word eneros_sel4_obj_tcb(void);
seL4_Word eneros_sel4_obj_endpoint(void);
seL4_Word eneros_sel4_obj_notification(void);
seL4_Word eneros_sel4_obj_captable(void);
/* seL4 aarch64 has no `seL4_ARM_FrameObject`; the 4 KiB user page (the
 * unit used to map an EL0 process image) is `seL4_ARM_SmallPageObject`. */
seL4_Word eneros_sel4_obj_arm_frame(void);
seL4_Word eneros_sel4_obj_arm_pt(void);
seL4_Word eneros_sel4_obj_arm_vspace(void);

/* seL4 cap rights. seL4_NoRights / seL4_CanRead / ... expand (via macro)
 * to `seL4_CapRights_new(...)` which returns a `seL4_CapRights_t` struct
 * `{ seL4_Uint64 words[1]; }`. We return the raw word (`words[0]`) so the
 * Rust side can treat rights as a plain `u64`. */
seL4_Word eneros_sel4_rights_none(void);
seL4_Word eneros_sel4_rights_read(void);
seL4_Word eneros_sel4_rights_write(void);
seL4_Word eneros_sel4_rights_grant(void);
seL4_Word eneros_sel4_rights_all(void);

/* ------------------------------------------------------------------ */
/* Debug serial output (seL4_DebugPutChar).                            */
/* ------------------------------------------------------------------ */
seL4_Word eneros_sel4_debug_put_char(char c);

/* ------------------------------------------------------------------ */
/* Boot info (seL4_GetBootInfo).                                       */
/* Returns a raw pointer; Rust treats it as opaque and only parses     */
/* fields it needs via offset accessors.                               */
/* ------------------------------------------------------------------ */
seL4_Word eneros_sel4_get_boot_info(void);

/* ------------------------------------------------------------------ */
/* Untyped retype (seL4_Untyped_Retype).                               */
/* seL4 14.0.0 signature (8 args):                                     */
/*   seL4_Untyped_Retype(_service, type, size_bits, root,              */
/*                       node_index, node_depth, node_offset,          */
/*                       num_objects)                                  */
/* `size_bits` is the log2 object size (e.g. 12 for a 4 KiB page).     */
/* ------------------------------------------------------------------ */
seL4_Error eneros_sel4_untyped_retype(seL4_CPtr service,
                                      seL4_ObjectType dest_type,
                                      seL4_Word size_bits,
                                      seL4_Word dest_count,
                                      seL4_CPtr dest_root,
                                      seL4_Word dest_index,
                                      seL4_Word dest_depth,
                                      seL4_Word offset);

/* ------------------------------------------------------------------ */
/* CNode operations (seL4_CNode_*).                                    */
/* ------------------------------------------------------------------ */
seL4_Error eneros_sel4_cnode_copy(seL4_CPtr dest_root,
                                  seL4_Word dest_index,
                                  seL4_Word dest_depth,
                                  seL4_CPtr src_root,
                                  seL4_Word src_index,
                                  seL4_Word src_depth,
                                  seL4_CapRights_t rights);

seL4_Error eneros_sel4_cnode_mint(seL4_CPtr dest_root,
                                  seL4_Word dest_index,
                                  seL4_Word dest_depth,
                                  seL4_CPtr src_root,
                                  seL4_Word src_index,
                                  seL4_Word src_depth,
                                  seL4_CapRights_t rights,
                                  seL4_Word badge);

seL4_Error eneros_sel4_cnode_move(seL4_CPtr dest_root,
                                  seL4_Word dest_index,
                                  seL4_Word dest_depth,
                                  seL4_CPtr src_root,
                                  seL4_Word src_index,
                                  seL4_Word src_depth);

seL4_Error eneros_sel4_cnode_delete(seL4_CPtr root,
                                    seL4_Word index,
                                    seL4_Word depth);

seL4_Error eneros_sel4_cnode_revoke(seL4_CPtr root,
                                    seL4_Word index,
                                    seL4_Word depth);

/* ------------------------------------------------------------------ */
/* IPC (seL4_SetMR / GetMR / Send / Recv / Call / ReplyRecv).          */
/* The seL4 aarch64 ABI passes the first N message words in registers  */
/* (x1..x7) and the rest in the IPC buffer; libsel4's inline functions */
/* handle this. The shim exposes them as real symbols.                 */
/* ------------------------------------------------------------------ */
void eneros_sel4_set_mr(seL4_Word i, seL4_Word mr);
seL4_Word eneros_sel4_get_mr(seL4_Word i);

/* seL4_Send is send-only (no reply) and returns void; Recv/Call/ReplyRecv
 * return the reply MessageInfo. */
void eneros_sel4_send(seL4_CPtr dest, seL4_MessageInfo_t msg_info);
seL4_MessageInfo_t eneros_sel4_recv(seL4_CPtr src, seL4_Word *sender);
seL4_MessageInfo_t eneros_sel4_call(seL4_CPtr dest, seL4_MessageInfo_t msg_info);
seL4_MessageInfo_t eneros_sel4_reply_recv(seL4_CPtr dest, seL4_MessageInfo_t msg_info, seL4_Word *sender);

/* MessageInfo packing (seL4_MessageInfo_new). */
seL4_MessageInfo_t eneros_sel4_message_info_new(seL4_Word label,
                                                seL4_Word caps_unwrapped,
                                                seL4_Word extra_caps,
                                                seL4_Word length);

/* ------------------------------------------------------------------ */
/* TCB operations (seL4_TCB_*).                                        */
/* These are what G6 AGT-003 needs to spawn a real EL0 thread (not a   */
/* Box<dyn>): WriteRegisters sets the entry point + SP, Resume starts  */
/* it, Suspend/Resume implement stop/kill.                             */
/* ------------------------------------------------------------------ */
seL4_Error eneros_sel4_tcb_write_registers(seL4_CPtr tcb,
                                           seL4_Bool resume_target,
                                           seL4_Word flags,
                                           seL4_Word count,
                                           const seL4_UserContext *regs);

seL4_Error eneros_sel4_tcb_read_registers(seL4_CPtr tcb,
                                          seL4_Bool suspend_target,
                                          seL4_Word flags,
                                          seL4_Word count,
                                          seL4_UserContext *regs);

seL4_Error eneros_sel4_tcb_resume(seL4_CPtr tcb);
seL4_Error eneros_sel4_tcb_suspend(seL4_CPtr tcb);
/* seL4_TCB_SetAffinity is gated on CONFIG_ENABLE_SMP_SUPPORT. When the
 * kernel is built without SMP (the current qemu-arm-virt baseline), the
 * wrapper returns seL4_IllegalOperation so the symbol still links. */
seL4_Error eneros_sel4_tcb_set_affinity(seL4_CPtr tcb, seL4_Word affinity);
seL4_Error eneros_sel4_tcb_set_priority(seL4_CPtr tcb, seL4_CPtr authority, seL4_Word priority);

/* seL4_UserContext register count (platform-specific). */
seL4_Word eneros_sel4_user_context_n_registers(void);

/* ------------------------------------------------------------------ */
/* VSpace / page mapping (seL4_ARM_Page_* / VSpace_*).                 */
/* Needed to build an EL0 address space for a spawned Agent process.   */
/* ------------------------------------------------------------------ */
seL4_Error eneros_sel4_arm_page_map(seL4_CPtr page,
                                    seL4_CPtr vspace,
                                    seL4_Word vaddr,
                                    seL4_CapRights_t rights,
                                    seL4_Word attr);

seL4_Error eneros_sel4_arm_page_table_map(seL4_CPtr pt,
                                          seL4_CPtr vspace,
                                          seL4_Word vaddr,
                                          seL4_Word attr);

/* seL4_ARM_Page default attributes getter. */
seL4_Word eneros_sel4_arm_default_attributes(void);

/* K2.3：把 notification 绑到 TCB（服务故障时内核发通知给绑定者）。 */
seL4_Error eneros_sel4_tcb_bind_notification(seL4_CPtr tcb, seL4_CPtr ntfn);

/* ------------------------------------------------------------------ */
/* G-KERNEL K1.2：真实 spawn 缺口（ADR-0034）。                        */
/* ------------------------------------------------------------------ */
seL4_Error eneros_sel4_tcb_set_space(seL4_CPtr tcb,
                                     seL4_Word fault_ep,
                                     seL4_CPtr cspace_root,
                                     seL4_Word cspace_root_data,
                                     seL4_CPtr vspace_root,
                                     seL4_Word vspace_root_data);

seL4_Error eneros_sel4_tcb_set_ipcbuffer(seL4_CPtr tcb,
                                         seL4_CPtr buffer,
                                         seL4_Word buffer_data);

seL4_Error eneros_sel4_arm_asid_pool_assign(seL4_CPtr pool,
                                            seL4_CPtr vspace);

seL4_Error eneros_sel4_arm_page_unmap(seL4_CPtr page);

seL4_Word eneros_sel4_cap_init_thread_tcb(void);
seL4_Word eneros_sel4_cap_init_thread_cnode(void);
seL4_Word eneros_sel4_cap_init_thread_vspace(void);
seL4_Word eneros_sel4_cap_init_thread_asid_pool(void);

/* WP-26.8 + 30 RT-015：IRQ 子系统绑定（移入 header guard——旧实现位于
 * #endif 之后，重复包含时守卫失效；若后续在声明区加宏定义将重复定义）。 */
seL4_Error eneros_sel4_irq_control_get(seL4_CPtr irq_ctrl, seL4_Word irq,
                                       seL4_CPtr dest_root, seL4_Word dest_index,
                                       seL4_Word dest_depth);
seL4_Error eneros_sel4_irq_handler_set_notification(seL4_CPtr handler, seL4_CPtr ntfn);
seL4_Error eneros_sel4_irq_handler_ack(seL4_CPtr handler);
seL4_Error eneros_sel4_irq_handler_clear(seL4_CPtr handler);
seL4_Word eneros_sel4_cap_irq_control(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* ENEROS_SEL4_SHIM_H */
