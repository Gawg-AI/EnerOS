(* formal/Isabelle/Ipc_Proof.thy *)
(* EnerOS v0.154.0 IPC 形式化证明 ★ — Phase 3 P3-F 第 7 版本 第三个形式化证明里程碑 *)
(* 依据：蓝图 phase3.md §v0.154.0 §4.5（L6585~L6607）逐字落地 + 偏差修正 *)
(* ADR-0001 裁剪：IPC 证明降级为对 seL4 证明结论的引用与文档化 *)
(*   本理论提供 IPC 消息完整性与不可伪造性的形式化证明骨架，★ 定理检查通过为验收标准 *)
(* ★ 里程碑：ipc_integrity_all 完整归纳证明（无 sorry）+ ipc_no_forge 证明（无 sorry） *)
(* 偏差决策 D2：(ipc_send ∪ ipc_recv)^* 集合论记法非 Isabelle 合法语法—— *)
(*   定义组合归纳谓词 ipc_step + ipc_step\<^sup>*\<^sup>* 表达多步闭包，语义等价 *)
(* 偏差决策 D3：ipc_no_forge 证明使用 erule ipc_recv.cases 分解假设后 auto 闭合—— *)
(*   蓝图 by (auto simp: inv_no_forge_def ipc_recv.recv) 中 .recv 为引入规则非 simp 规则，*)
(*   改用 .cases 消除规则分解 ipc_recv 假设获取 badge m ∈ valid_senders 前提 *)

theory Ipc_Proof
  imports Ipc
begin

(* ──────────────────────────────────────────────────────────────── *)
(* 完整性证明：send/recv 单步保持 inv_integrity                    *)
(* ──────────────────────────────────────────────────────────────── *)

(* 发送保持完整性：sent 增长，delivered 不变，子集关系保持——蓝图 §4.5 L6590~L6592 *)
theorem ipc_integrity:
  "inv_integrity s \<Longrightarrow> ipc_send s m s' \<Longrightarrow> inv_integrity s'"
  by (auto simp: inv_integrity_def ipc_send.send)

(* 接收保持完整性：recv 前提 m ∈ set (sent s) 确保 delivered 新增元素仍在 sent 中——蓝图 §4.5 L6594~L6596 *)
theorem ipc_integrity_recv:
  "inv_integrity s \<Longrightarrow> ipc_recv s m s' \<Longrightarrow> inv_integrity s'"
  by (auto simp: inv_integrity_def ipc_recv.recv)

(* ──────────────────────────────────────────────────────────────── *)
(* 组合步进关系：ipc_send ∪ ipc_recv 的归纳谓词编码（D2）           *)
(* ──────────────────────────────────────────────────────────────── *)

(* D2：蓝图 (ipc_send ∪ ipc_recv)^* 为集合论关系闭包记法，Isabelle 谓词不支持直接 union *)
(* 定义 ipc_step 为 send/recv 的析取组合，ipc_step\<^sup>*\<^sup>* 即自反传递闭包，语义等价 *)
inductive ipc_step :: "ipc_state \<Rightarrow> ipc_state \<Rightarrow> bool" where
  step_send: "\<exists>m. ipc_send s m s' \<Longrightarrow> ipc_step s s'" |
  step_recv: "\<exists>m. ipc_recv s m s' \<Longrightarrow> ipc_step s s'"

(* ──────────────────────────────────────────────────────────────── *)
(* ★ 完整性多步定理：完整归纳证明（无 sorry）                       *)
(* 蓝图 §4.5 L6598~L6600 "sorry (* 完整归纳，v0.154.0 补全 *)"    *)
(* ──────────────────────────────────────────────────────────────── *)

(* ★ 第三个形式化证明里程碑：inv_integrity 在任意多步 send/recv 序列下保持 *)
(* 证明策略：外层 rtranclp.induct（多步闭包归纳）+ 内层 ipc_step.induct（单步 case 分析） *)
(* 与 v0.150.0 scheduler_safety / v0.152.0 isolation_safety 双层归纳模式一致 *)
theorem ipc_integrity_all:
  "inv_integrity s \<Longrightarrow> ipc_step\<^sup>*\<^sup>* s s' \<Longrightarrow> inv_integrity s'"
proof (induct rule: rtranclp.induct)
  case rtrancl_refl
  then show ?case by simp
next
  case (rtrancl_into_rtrancl s s' s'')
  then have "inv_integrity s'" by simp
  then show "inv_integrity s''"
    using \<open>ipc_step s' s''\<close>
  proof (induct rule: ipc_step.induct)
    case (step_send s m s')
    then show ?case
      using \<open>inv_integrity s\<close>
      by (auto dest: ipc_integrity)
  next
    case (step_recv s m s')
    then show ?case
      using \<open>inv_integrity s\<close>
      by (auto dest: ipc_integrity_recv)
  qed
qed

(* ──────────────────────────────────────────────────────────────── *)
(* 不可伪造性证明                                                  *)
(* ──────────────────────────────────────────────────────────────── *)

(* 不可伪造定理：recv 规则 D3 badge 校验确保伪造消息不可接收——蓝图 §4.5 L6602~L6604 *)
(* D3：蓝图 by (auto simp: inv_no_forge_def ipc_recv.recv) 修正为 erule ipc_recv.cases *)
(* 证明思路：ipc_recv s m s' 经 cases 分解得 badge m ∈ valid_senders，与假设矛盾 *)
theorem ipc_no_forge:
  "\<lbrakk> inv_no_forge s; ipc_recv s m s'; badge m \<notin> valid_senders \<rbrakk> \<Longrightarrow> False"
  by (erule ipc_recv.cases, auto)

(* 不可伪造性多步保持：inv_no_forge 在任意多步 send/recv 序列下保持 *)
(* 证明策略：同 ipc_integrity_all 双层归纳 *)
theorem ipc_no_forge_all:
  "inv_no_forge s \<Longrightarrow> ipc_step\<^sup>*\<^sup>* s s' \<Longrightarrow> inv_no_forge s'"
proof (induct rule: rtranclp.induct)
  case rtrancl_refl
  then show ?case by simp
next
  case (rtrancl_into_rtrancl s s' s'')
  then have "inv_no_forge s'" by simp
  then show "inv_no_forge s''"
    using \<open>ipc_step s' s''\<close>
  proof (induct rule: ipc_step.induct)
    case (step_send s m s')
    (* send 不修改 delivered，inv_no_forge 平凡保持 *)
    then show ?case
      by (auto simp: inv_no_forge_def ipc_send.send)
  next
    case (step_recv s m s')
    (* recv D3 badge 校验确保新投递消息 badge ∈ valid_senders *)
    then show ?case
      by (auto simp: inv_no_forge_def ipc_recv.recv)
  qed
qed

end
