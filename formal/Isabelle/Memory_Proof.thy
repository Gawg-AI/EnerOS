(* formal/Isabelle/Memory_Proof.thy *)
(* EnerOS v0.152.0 内存隔离形式化证明 ★ — Phase 3 P3-F 第 5 版本 第二个形式化证明里程碑 *)
(* 依据：蓝图 phase3.md §v0.152.0 §4.5（L6415~L6445）逐字落地 + 偏差修正 *)
(* ADR-0001：内存隔离证明为 2 项保留增量证明之一（#2 内存），#1 调度器证明已 v0.150.0 完成 *)
(* ★ 里程碑：isolation_safety 完整归纳证明，无 sorry（机械检查通过为验收标准） *)
(* 偏差决策 D2a：map_page^* → map_page\<^sup>*\<^sup>*（predicate rtranclp 记法，蓝图 set-based 记法错误修正） *)
(*   与 v0.149.0 liv_fair / v0.150.0 scheduler_safety 记法一致 *)
(* 偏差决策 D2b：case (map v s target s') → case (map v s target)（去除多余 s'） *)
(*   map_page 的 map 规则自由变量仅 v/s/target，s' 为派生状态 s\<lparr>partitions := add_region s target v\<rparr> *)
(*   Isabelle inductive case 变量由规则自由变量生成，派生项非 case 变量 *)
(* 偏差决策 D2c：(rule isolation_safety) 自引用 → then have "inv_isolation s'" by simp（直接引用 IH） *)
(*   rtranclp.induct 的 rtrancl_into_rtrancl case 已提供归纳假设 IH，避免自引用正在证明的定理 *)
(*   与 v0.150.0 scheduler_safety 的 then have "inv_priority s'" by simp 一致 *)
(* 偏差决策 D2d/D2e：simp 规则补充 add_region_def（蓝图仅 accessible_def 不足，add_region 为 v0.151.0 D3 新增） *)
(* 偏差决策 D3：Memory.thy 的 map_preserves_isolation sorry 保留不动（Surgical Changes） *)
(*   isolation_safety（rtranclp 多步）逻辑蕴含 map_preserves_isolation（单步），★ 验收针对 isolation_safety *)
(*   与 v0.150.0 一致（Scheduler.thy 的 step_preserves_priority sorry 保留，Scheduler_Proof.thy 交付 scheduler_safety 无 sorry） *)

theory Memory_Proof
  imports Memory
begin

(* 安全性定理：隔离不变式在所有映射操作（自反传递闭包）保持——蓝图 §4.5 L6420~L6442 *)
(* ★ 第二个形式化证明里程碑：完整归纳证明，无 sorry *)
(* 完整归纳：外层 rtranclp.induct（自反传递闭包）+ 内层 map_page.induct（单步映射） *)
(* D2a 修正：map_page\<^sup>*\<^sup>* 记法（predicate rtranclp，非 set-based rtrancl） *)
theorem isolation_safety:
  "inv_isolation s \<Longrightarrow> map_page\<^sup>*\<^sup>* s s' \<Longrightarrow> inv_isolation s'"
proof (induct rule: rtranclp.induct)
  case rtrancl_refl
  then show ?case by simp
next
  case (rtrancl_into_rtrancl s s' s'')
  (* D2c 修正：直接引用归纳假设 IH（rtranclp.induct 提供），避免 (rule isolation_safety) 自引用 *)
  then have "inv_isolation s'" by simp
  then show "inv_isolation s''"
    using \<open>map_page s' s''\<close>
  proof (induct rule: map_page.induct)
    (* D2b 修正：去除多余 s'——map 规则自由变量仅 v/s/target *)
    case (map v s target)
    have "v \<notin> \<Union> {accessible s p | p. p \<noteq> target}" by fact
    then have "accessible s' target = accessible s target \<union> {v}"
      by (simp add: accessible_def add_region_def)
    moreover have "\<forall>p. p \<noteq> target \<longrightarrow> accessible s' p = accessible s p"
      by (simp add: accessible_def add_region_def)
    ultimately show "inv_isolation s'"
      using \<open>inv_isolation s\<close>
      by (auto simp: inv_isolation_def)
  qed
qed

end
