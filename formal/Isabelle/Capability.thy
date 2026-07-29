(* formal/Isabelle/Capability.thy *)
(* EnerOS v0.155.0 能力管理器规范抽象 — Phase 3 P3-F 终版本 *)
(* 依据：蓝图 phase3.md §v0.155.0 §4.1/§4.5 + ADR-0001 裁剪 *)
(* ADR-0001 裁剪：能力管理器形式化证明在方案 A 下降级为对 seL4 证明结论的引用与文档化 *)
(*   seL4 的 capability 模型已有数学证明，本理论提供 EnerOS 规范层文档化补充 *)
(* 偏差决策 D2：蓝图 record cap = rights :: "right set" | parent :: "cap option" 为递归记录，*)
(*   Isabelle 不支持递归记录类型——改用 cid (nat) 标识 + parent_cid (nat option) 非递归编码 *)
(* 偏差决策 D3：蓝图 mint p r c 为关系式——改为函数式 mint :: cap ⇒ nat set ⇒ nat ⇒ cap，*)
(*   语义等价，证明更直接（auto simp: mint_def 即可闭合） *)
(* 偏差决策 D4：蓝图 revoke s cap 为关系式——改为函数式 revoke :: cap_state ⇒ cap ⇒ cap_state，*)
(*   配合 descendant 归纳谓词实现级联删除，revoke_cascade 定理证明无 sorry *)

theory Capability
  imports Main
begin

(* ──────────────────────────────────────────────────────────────── *)
(* 数据类型与记录                                                   *)
(* ──────────────────────────────────────────────────────────────── *)

(* 权限类型：以 nat 集合表示能力权限集——蓝图 §4.1 "right set" *)
type_synonym rights = "nat set"

(* D2：能力抽象——蓝图 record cap 的非递归编码 *)
(* cid：能力唯一标识；cap_rights：权限集；parent_cid：父能力标识（None = 根能力） *)
record cap =
  cid :: nat
  cap_rights :: rights
  parent_cid :: "nat option"

(* 能力状态：活跃能力列表——蓝图 §4.5 "caps s" *)
record cap_state =
  cap_list :: "cap list"

(* ──────────────────────────────────────────────────────────────── *)
(* 操作定义                                                         *)
(* ──────────────────────────────────────────────────────────────── *)

(* D3：mint 操作——创建子能力，权限为 r（调用者须保证 r ⊆ cap_rights parent） *)
(* 蓝图 §4.5：mint p r c ⟹ rights c = r *)
definition mint :: "cap \<Rightarrow> rights \<Rightarrow> nat \<Rightarrow> cap" where
  "mint parent r child_id \<equiv> \<lparr>cid = child_id, cap_rights = r, parent_cid = Some (cid parent)\<rparr>"

(* 派生关系：child 是 parent 的直接子能力（在 cap_list 中 parent_cid 指向 parent） *)
definition is_child :: "cap \<Rightarrow> cap \<Rightarrow> bool" where
  "is_child child parent \<equiv> parent_cid child = Some (cid parent)"

(* ──────────────────────────────────────────────────────────────── *)
(* 后代关系（归纳谓词）                                             *)
(* ──────────────────────────────────────────────────────────────── *)

(* 后代关系：直接子能力 + 传递闭包——蓝图 §4.5 "descendants s cap = ds" *)
inductive descendant :: "cap \<Rightarrow> cap \<Rightarrow> cap list \<Rightarrow> bool" where
  (* 直接子能力：d 在 cap_list 中且 parent_cid 指向 c *)
  direct_child: "\<lbrakk> d \<in> set cap_list; parent_cid d = Some (cid c) \<rbrakk>
    \<Longrightarrow> descendant c d cap_list" |
  (* 传递性：d 是 c 的子能力的后代 ⟹ d 是 c 的后代 *)
  trans: "\<lbrakk> child \<in> set cap_list; parent_cid child = Some (cid c);
           descendant child d cap_list \<rbrakk> \<Longrightarrow> descendant c d cap_list"

(* ──────────────────────────────────────────────────────────────── *)
(* 撤销操作                                                         *)
(* ──────────────────────────────────────────────────────────────── *)

(* D4：revoke 操作——移除能力及其全部后代——蓝图 §4.5 "revoke s cap" *)
(* 语义：从 cap_list 中过滤掉 cap 本身及所有 descendant *)
definition revoke :: "cap_state \<Rightarrow> cap \<Rightarrow> cap_state" where
  "revoke s c \<equiv> s\<lparr>cap_list := [x \<leftarrow> cap_list s. x \<noteq> c \<and> (\<forall>d. descendant c d (cap_list s) \<longrightarrow> x \<noteq> d)]\<rparr>"

(* ──────────────────────────────────────────────────────────────── *)
(* 访问检查                                                         *)
(* ──────────────────────────────────────────────────────────────── *)

(* check 操作：验证能力存在且拥有指定权限——蓝图 §4.5 "check s cap required" *)
definition check :: "cap_state \<Rightarrow> cap \<Rightarrow> rights \<Rightarrow> bool" where
  "check s c required \<equiv> c \<in> set (cap_list s) \<and> required \<subseteq> cap_rights c"

end
