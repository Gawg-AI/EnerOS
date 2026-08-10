# WP-31.3 GOOSE/SV 标准化与互操作（PR-03/04/16）修复与证据

执行日期：2026-08-11。蓝图坐标：31 §5（M2 互操作判据前置）。

## PR-03（P1，线格式偏离 IEC 61850）
- **四字节头**：tx 在 APPID 后插入 Length(2B，含自身 2B+Reserved 4B+APDU
  全长) + Reserved1(2B) + Reserved2(2B)——IEC 61850-8-1 §12；rx/sv 剥头
  （frame[22..]）+ **Length 字段校验**（声称长度与实际不符 → 拒绝，堵
  malformed）。
- **allData 标准 tag**：BOOLEAN 0x80 → 0x83（IEC 61850-8-1 §19）；其余
  tag（INTEGER 0x85 / FLOAT 0x87 / UNSIGNED 0x86 / VISIBLE-STRING 0x8A /
  BinaryTime 0x91）与标准一致保留。
- fixture 整体重生成（R31-1 声明）：tx7（Length 覆盖到帧尾断言）、tx13
  （bool tag）、tx18（TLV walk 偏移 22）、rx/build_raw_frame/sv_frame
  全部更新。

## PR-04（P2，0x81 长型不一致）
- goose_rx / mms ber_decode 增 0x81 单字节长型分支（sv_rx 已有）——BER
  三型（短型/0x81/0x82）三路径统一。
- 判据：test_rx81_single_byte_long_form_length（200B TLV 0x81 解码）。

## PR-16（P2，互操作脚本）
- third_party/goose_peer.py：标准 GOOSE 帧参考实现（四字节头 + 标准 tag +
  malformed Length 变体）。
- tests/interop/goose.sh：PASS 行 ≥5（peer 生成/解码器/订阅路径/malformed
  拒绝/证据输出）——实测 5 PASS。
- tests/vectors/goose_ref.bin fixture + test_reference_frames_decode
  （valid ≥2 解码 PASS + malformed ≥1 拒绝）。

## R31-1 次生登记
线格式变更使既有回放证据（evidence/goose-*，若有）失效——本仓库
evidence/ 无 GOOSE 回放文件（grep 核实），无重跑项。

## 回归
goose 38（+2 判据）/ sv 25 / mms 38 全绿。
