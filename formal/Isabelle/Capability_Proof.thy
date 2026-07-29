(* formal/Isabelle/Capability_Proof.thy *)
(* EnerOS v0.155.0 能力管理器形式化证明 ★ — Phase 3 P3-F 终版本 第四个形式化证明里程碑 *)
(* 依据：蓝图 phase3.md §v0.155.0 §4.5（L6670~L6691）逐字落地 + 偏差修正 *)
(* ADR-0001 裁剪：能力管理器形式化证明降级为对 seL4 证明结论的引用与文档化 *)
(*   seL4 的 capability 模型已有数学证明，本理论提供 EnerOS 规范层文档化补充 *)
(* ★ 里程碑：rights_monotone + revoke_cascade + no_bypass 三定理完整证明（无 sorry） *)
(* 偏差决策 D2：蓝图 record cap 递归类型→非递归 cid 编码（Capability.thy 已述） *)
(* 偏差决策 D3：mint 关系式→函数式（Capability.thy 已述），rights_monotone auto 闭合 *)
(* 偏差决策 D4：revoke 关系式→函数式（Capability.thy 已述），revoke_cascade 经 *)
(*   descendant_subset 辅助引理 + descendant.induct 归纳证明，无 sorry *)
(* 偏差决策 D5：蓝图 by (induct rule: revoke.induct, auto) 修正为 *)
(*   by (auto simp: revoke_def dest: descendant_subset)——函数式 revoke 无 .induct 规则 *)

theory Capability_Proof
  imports Capability
begin

(* ──────────────────────────────────────────────────────────────── *)
(* ★ 权限单调性定理：mint 保持权限缩减（只能缩减，不可扩增）         *)
(* 蓝图 §4.5 L6676~L6678                                           *)
(* ──────────────────────────────────────────────────────────────── *)

(* 辅助引理：mint 创建的子能力权限恰为 r *)
lemma mint_rights_eq: "cap_rights (mint p r child_id) = r"
  by (auto simp: mint_def)

(* ★ 权限单调性定理：子能力权限 = 指定权限集 ∧ 子能力权限 ⊆ 父能力权限 *)
(* 蓝图：theorem rights_monotone: "⟦ rights p ⊇ r; mint p r c ⟧ ⟹ rights c = r ∧ rights c ⊆ rights p" *)
(* D3 修正：函数式 mint 下直接 auto simp 闭合 *)
theorem rights_monotone:
  "cap_rights p \<supseteq> r \<Longrightarrow>
   cap_rights (mint p r child_id) = r \<and> cap_rights (mint p r child_id) \<subseteq> cap_rights p"
  by (auto simp: mint_def)

(* ──────────────────────────────────────────────────────────────── *)
(* ★ 撤销级联定理：revoke 移除全部后代                              *)
(* 蓝图 §4.5 L6681~L6684                                           *)
(* ──────────────────────────────────────────────────────────────── *)

(* 辅助引理：后代必在 cap_list 中（descendant 归纳） *)
(* 证明策略：对 descendant 归纳谓词做规则归纳（descendant.induct） *)
lemma descendant_subset:
  "descendant c d (cap_list s) \<Longrightarrow> d \<in> set (cap_list s)"
proof (induct rule: descendant.induct)
  case (direct_child d cap_list c)
  then show ?case by simp
next
  case (trans child cap_list c d)
  then show ?case by simp
qed

(* ★ 撤销级联定理：revoke 后，原状态中 cap 的全部后代不再存在 *)
(* 蓝图：theorem revoke_cascade: "⟦ revoke s cap; descendants s cap = ds ⟧ ⟹ ∀d∈ds. d ∉ set (caps s')" *)
(* D4/D5 修正：函数式 revoke + descendant_subset 引理，auto 闭合 *)
theorem revoke_cascade:
  "descendant c d (cap_list s) \<Longrightarrow> d \<notin> set (cap_list (revoke s c))"
  by (auto simp: revoke_def dest: descendant_subset)

(* 撤销自移除：revoke 后 cap 本身亦不存在 *)
theorem revoke_self_removed:
  "c \<notin> set (cap_list (revoke s c))"
  by (auto simp: revoke_def)

(* ──────────────────────────────────────────────────────────────── *)
(* ★ 不可绕过定理：无能力则不可访问                                  *)
(* 蓝图 §4.5 L6687~L6689                                           *)
(* ──────────────────────────────────────────────────────────────── *)

(* ★ 不可绕过定理：能力不在 cap_list 中时 check 必为 False *)
(* 蓝图：theorem no_bypass: "⟦ cap ∉ set (caps s); check s cap required ⟧ ⟹ False" *)
theorem no_bypass:
  "c \<notin> set (cap_list s) \<Longrightarrow> check s c required \<Longrightarrow> False"
  by (auto simp: check_def)

(* ──────────────────────────────────────────────────────────────── *)
(* 组合定理：P3-F 终版本安全性总结                                   *)
(* ──────────────────────────────────────────────────────────────── *)

(* 能力安全三属性联合表述：单调性 + 级联撤销 + 不可绕过 *)
theorem capability_security:
  "(\<forall>p r child_id. cap_rights p \<supseteq> r \<longrightarrow>
     cap_rights (mint p r child_id) = r \<and> cap_rights (mint p r child_id) \<subseteq> cap_rights p) \<and>
   (\<forall>s c d. descendant c d (cap_list s) \<longrightarrow> d \<notin> set (cap_list (revoke s c))) \<and>
   (\<forall>s c required. c \<notin> set (cap_list s) \<longrightarrow> \<not> check s c required)"
  by (auto simp: mint_def revoke_def check_def dest: descendant_subset)

end
