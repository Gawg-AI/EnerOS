(* formal/Isabelle/Ipc.thy *)
(* EnerOS v0.153.0 IPC 规范抽象 — Phase 3 P3-F 第 6 版本 *)
(* 依据：蓝图 phase3.md §v0.153.0 §4.5（L6504~L6527）逐字落地 + 偏差修正 *)
(* ADR-0001 裁剪：IPC 证明在方案 A 下降级为对 seL4 证明结论的引用与文档化 *)
(*   本理论提供 IPC 消息传递的形式化规范（完整性 + 不可伪造性），为 v0.154.0 证明提供输入 *)
(* 偏差决策 D2：valid_senders 常量蓝图未显式声明——新增 consts valid_senders :: "nat set" *)
(* 偏差决策 D3：recv 规则蓝图仅含 m ∈ set (sent s) 前提——追加 badge m ∈ valid_senders 前提 *)
(*   理由：若 recv 不校验 badge，则 inv_no_forge 不可归纳保持（反例：发送 badge ∉ valid_senders 的消息后接收） *)
(*   蓝图 §4.4 错误处理「加强 badge 校验」即指此修正 *)

theory Ipc
  imports Kernel
begin

(* 消息抽象：内容 + badge（发送方标识）——蓝图 §4.5 L6509 *)
record msg =
  content :: "nat list"
  badge :: nat

(* D2：合法发送方集合——蓝图 inv_no_forge 引用 valid_senders 但未声明 *)
(* 语义：系统配置的合法发送方 badge 集合，recv 规则校验 badge 归属 *)
consts valid_senders :: "nat set"

(* IPC 状态：已发送消息列表 + 已投递消息列表——蓝图 §4.5 L6510 *)
record ipc_state =
  sent :: "msg list"
  delivered :: "msg list"

(* 完整性不变式：接收到的消息必曾发送（delivered ⊆ sent）——蓝图 §4.5 L6513~L6514 *)
definition inv_integrity :: "ipc_state \<Rightarrow> bool" where
  "inv_integrity s \<equiv> set (delivered s) \<subseteq> set (sent s)"

(* 不可伪造不变式：已投递消息的 badge 必属合法发送方——蓝图 §4.5 L6517~L6518 *)
definition inv_no_forge :: "ipc_state \<Rightarrow> bool" where
  "inv_no_forge s \<equiv> \<forall>m\<in>set (delivered s). badge m \<in> valid_senders"

(* 发送操作：消息入队 sent——蓝图 §4.5 L6520~L6521 *)
inductive ipc_send :: "ipc_state \<Rightarrow> msg \<Rightarrow> ipc_state \<Rightarrow> bool" where
  send: "ipc_send s m (s\<lparr>sent := m # sent s\<rparr>)"

(* 接收操作：从 sent 取消息投递到 delivered——蓝图 §4.5 L6523~L6524 *)
(* D3：追加 badge 校验前提——确保 inv_no_forge 可归纳保持（蓝图 §4.4 加强 badge 校验） *)
inductive ipc_recv :: "ipc_state \<Rightarrow> msg \<Rightarrow> ipc_state \<Rightarrow> bool" where
  recv: "\<lbrakk> m \<in> set (sent s); badge m \<in> valid_senders \<rbrakk> \<Longrightarrow>
    ipc_recv s m (s\<lparr>delivered := m # delivered s\<rparr>)"

end
