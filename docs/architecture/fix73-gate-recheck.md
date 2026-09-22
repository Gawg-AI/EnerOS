# 蓝图 73 批B · 归档卷宗复判报告（门判据登记化后的新旧结论对照）

> 日期：2026-09-22（批B 施工会话）。
> 对象：判据收编（`tools/gate-criteria.toml` + `tools/criterion.py`）后，用**新判据器**
> 复判历史归档卷宗——S3：零意外漂移；任何翻红必须停下报 owner（定案点①），
> 不得为保绿放宽登记。
> 输入只读（C6）：`evidence/` 零写入；本报告是唯一产物。
> 复判命令形态：`python3 tools/criterion.py check <id> <archive-file>`；旧值 =
> 收编前脚本判据行的逐字重放（bash 同形 grep）。

## 1. 蓝图 70 卷 70.5（身份/信任卷，两窗串口）

| 卷宗 | 判据 | 旧判定（收编前逻辑） | 新判定（登记判据） | 结论 |
|---|---|---|---|---|
| `wa.qemu-serial.log` | R9 | 裸子串 `IDENTITY` ×4 → ✓（**4 行全为 BOOT-IDENTITY-PAGE / IDENTITY-PUSH: OK / OTA-IDENTITY / BOOT-IDENTITY 词形，真 token 也在场**） | any_of 真命中 ×1（IDENTITY-PUSH: OK）+ none_of FAILED=0 → `[✓][R9] 窗B 身份面观测（IDENTITY ×1）` | **PASS → PASS（不翻红；计数 4→1 = 虚胖现真值）** |
| `wa.qemu-serial.log` | R9b | `OTA-CONFIG gen=` ×1 → ✓ | ×1 → ✓ | PASS → PASS |
| `wa.qemu-serial.log` | PANIC | `-i "PANIC\|memory allocation of"` 0 → ✓ | PANIC-SCAN（大小写敏感/全文件域）0 → ✓ | PASS → PASS |
| `wb.qemu-serial.log` | R9 | 裸子串 ×4 → ✓（同 wa：真 token 在场） | ×1 → ✓ | PASS → PASS |
| `wb.qemu-serial.log` | R9b | ×1 → ✓ | ×1 → ✓ | PASS → PASS |
| `wb.qemu-serial.log` | PANIC | 0 → ✓ | 0 → ✓ | PASS → PASS |

注（假绿面的归档实证随卷）：同一裸子串对**只含** BOOT-IDENTITY-PAGE 等无关
行的恶意卷宗判 ✓（`tools/testdata/gate-false-green/repro-current.txt`）——归档
两窗恰因真 token 在场而未被骗；新判据下该风险关闭。

## 2. 蓝图 70 卷 70.15 R10-HELD（B 窗：脏断电重启续接）

| 复判分量 | 旧判定（收编前逻辑） | 新判定（登记判据） | 结论 |
|---|---|---|---|
| B4 `NDELIV`（`EXEC-JOURNAL applied-confirmed` @ `win-b.qemu-serial.log`） | grep ×0 | `check R10-B4-DELIVERY --count-only` = 0 | **一致（0 → 0）** |
| B4 `fc06`（归档 `win-b.modbus-transcript.jsonl`） | ×5（整场转写，含割切前） | 同函数 ×5（`FileNotFoundError→exit-3` 仅影响缺输入路径，在场文件计数不变） | **一致（5 → 5）** |
| B4 判定 | `FC06 ×0 ≤ journal ×0` → ✓（§11.14 在案） | N6=0（割切后零新写）≤ NDELIV=0（重启窗零新投递）→ ✓ | **PASS → PASS（不翻红）** |
| B5（PANIC 第三词表 @ `win-b.qemu-serial.log`） | `PANIC\|alloc.*failed\|ALLOC-FAIL` 0 → ✓ | PANIC-SCAN 0 → ✓ | **PASS → PASS** |

**B4 `0 ≤ 0` 的语义注记（非缺输入假绿）**：归档 B 窗的"零 FC06 新写 +
零 EXEC-JOURNAL 新投递"是 §11.14 实弹的**真实形态**——割切重启后未发步
全部 Blocked（B2）/cancelled（B3），无新执行；`win-b-precut.qemu-serial.log`
含 5 条 applied-confirmed（割切前缀），`win-b.modbus-transcript.jsonl` 的
fc06 ×5 同属割切前——差值 N6=0 与串口 NDELIV=0 自洽。缺输入=红的
exit-3 契约针对**未来运行**的输入缺席（本批 B-P0 已证旧逻辑在该形态
假绿）；归档卷宗 T1（pre-cut 转写）未随卷归档，其运行期在场由同窗
B0 witness（`r10_b_witness.py` 消费 `win-b-precut.qemu-serial.log`）背书。

## 3. 结论

- **全部复判项新旧结论一致，零翻红** ⇒ 定案点①不触发；登记未做任何
  为保绿的放宽。
- 定案点②（R9 any_of 语义重述：`IDENTITY-PUSH: OK ∣ synthetic`）经两窗
  归档复判 PASS 确认不构成行为回退；如 owner 改判 qemu-test 窗必须独占
  synthetic 形，改 `gate-criteria.toml` 一行即可（机械调整，不动代码）。
- 本报告与 §11.15（deepening-blueprint-v1.md）互为指针。
