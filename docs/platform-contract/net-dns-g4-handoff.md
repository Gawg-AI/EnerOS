# NET-003 DNS Resolver Hardening（G4 移交规格）

> 阶段：G3 完成 NET-001（socket IPC 两进程真实收发）与 NET-002 link config
> （virtio-net MAC/MTU 读取）；NET-003（DNS resolver hardening）按 MASTER
> 归 G4（网络、密码与工业协议），本文件冻结其验收规格作为 G4 输入。

## 目标

DNS resolver 必须对抗：伪造应答（spoofing）、响应超时、缓存边界溢出、投毒。

## 验收规格（G4 执行）

1. **spoofing**：仅接受匹配 query ID + 源端口 + 源地址的应答；随机 ID/端口；
   重复 query 去重。
2. **timeout**：单次查询超时（默认 5s）→ 重试（最多 3 次）→ 失败降级（缓存或
   明确错误），不得无限阻塞。
3. **cache bounds**：缓存条目数/生存期上限；LRU 淘汰；条目大小与总内存上限；
   恶意超大应答拒绝。
4. **投毒防护**：DNSSEC 可选校验；应答与 query 域名精确匹配；CNAME 链深度上限。
5. **负测试**：伪造 ID/乱序/重放/畸形压缩指针（pointer loop）全部拒绝。

## 依赖

- G4 NET-002 完整 TCP/IP 栈（IPv4/IPv6/transport）。
- G4 审计过的 TLS/证书基础（时间可信）。

## 证据要求

- 独立 resolver 对端互操作正/负测试。
- 畸形/重放/超时故障注入日志与机器可解析结果。
