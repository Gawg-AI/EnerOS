# EnerOS v0.98.1 纵向加密对接指南与合规控制点矩阵

> **版本**：v0.98.1（刚性合规子版本）
> **蓝图**：phase2.md §v0.98.1（36 号文合规，Phase 2 安全合规出口条件）
> **Crate**：`eneros-federation`（`crates/agents/federation/src/tunnel.rs`）
> **配套**：`configs/vertical-encrypt.toml` / [vertical-encrypt-design.md](./vertical-encrypt-design.md)

本文档分两大部分：**第一部分《纵向加密对接指南》**——面向现场集成与运维，给出装置适配 seam、证书配置、隧道策略配置与现场互通验收步骤；**第二部分《合规控制点矩阵（纵向加密部分）》**——面向合规审查，逐项对照 36 号文及等保要求给出本实现机制与验收证据。

---

# 第一部分：纵向加密对接指南

## 1. 装置适配 seam（VerticalEncryptDevice trait 实现指引）

本实现不直接绑定任何具体纵向加密卡硬件，而是以 `VerticalEncryptDevice` sync trait 作为**装置适配 seam**（E2：驱动语义 = 帧收发 seam）。现场接入真实纵向加密卡时，由驱动工程师实现该 trait 并以 `Box<dyn VerticalEncryptDevice>` 注入，tunnel 层（IKE / 隧道收发 / 重放窗口 / 调度认证）**零改动**。

### 1.1 trait 契约

```rust
pub trait VerticalEncryptDevice {
    /// 发送帧：将完整隧道帧经加密卡物理通道发出；
    /// 卡硬件故障/链路断开 → Err(EncryptError::DeviceError)
    fn xmit(&mut self, frame: &[u8]) -> Result<(), EncryptError>;

    /// 轮询接收帧：非阻塞语义——有帧 → Ok(Some(frame))，
    /// 无帧 → Ok(None)，卡故障 → Err(EncryptError::DeviceError)
    fn poll(&mut self) -> Result<Option<Vec<u8>>, EncryptError>;
}
```

### 1.2 实现指引（现场驱动工程师）

| 步骤 | 要求 |
|------|------|
| ① 帧边界 | `xmit` 输入为**完整隧道帧**（`local_spi‖seq‖iv‖ct‖tag`），驱动不得修改帧内容；若卡硬件有 MTU 限制需分片，分片/重组在驱动内部完成，对 trait 消费者透明 |
| ② 非阻塞 poll | `poll` 必须非阻塞（no_std 单线程模型，阻塞会卡死 Agent Runtime 调度）；卡内无帧立即返回 `Ok(None)` |
| ③ 错误映射 | 卡硬件错误（链路断、FIFO 溢出、看门狗复位）统一映射为 `Err(DeviceError)`；**不得**将帧内容错误（如接收帧 CRC 错）映射为 DeviceError——格式问题由 tunnel_recv 以 `InvalidFrame` 判定 |
| ④ no_std 合规 | 驱动代码禁止 `std::*`（蓝图 §43.1）；缓冲区用 `alloc::vec::Vec`（Agent Runtime 分区有用户堆）或静态缓冲 |
| ⑤ 注入方式 | 参考 `MockVerticalEncryptDevice`（回环实现，CI 用）：`let dev: Box<dyn VerticalEncryptDevice> = Box::new(MyCardDriver::new(...));` |

### 1.3 Mock 回环（CI / 开发期）

CI 环境无硬件，以 `MockVerticalEncryptDevice` 回环替代：`xmitted` 记录已发帧、`pending` 预置待收帧、`fail_times` 故障注入（>0 时 xmit/poll 递减并 `Err(DeviceError)`）。双端回环互通测试（TV16~TV23）已验证隧道语义正确性；真实装置互通为**现场验收项 E10**（见 §4）。

## 2. 证书配置（SM2 证书来源：电网 PKI）

- **证书体系**：本端与对端（调度主站侧）SM2 证书均须来源于**电网调度 PKI**（调度机构统一签发），禁止自签名证书接入生产纵向通道；
- **证书载体**：IKE 层以 opaque bytes 传递证书（E4：不新造证书类型），证书链验证复用 v0.97.0 `CertVerifier` trait——**PKI v0.32.0 证书链适配器后续注入**后强制信任链校验（签名/有效期/信任链）；
- **密钥对**：本端 `Sm2KeyPair`（私钥不出安全载体）、对端 `Sm2PublicKey`（随证书发布）；IKE 协商时 PMS 以**对端公钥** SM2 加密传输，仅对端私钥可解密；
- **证书轮换**：隧道持有**派生密钥而非证书**（E12）——证书轮换天然不影响已有隧道连接；新证书生效后仅需在**下次 IKE 建隧**时使用，在线隧道无需重建；
- **私钥保护**：私钥不明文落盘、不入仓（`.gitignore` 已覆盖 `*.pem`/`*.key`）；PMS 预主密钥不出内存明文留存、派生后即弃。

## 3. 隧道策略配置（引用 configs/vertical-encrypt.toml）

隧道策略经 `configs/vertical-encrypt.toml` `[vertical_encrypt]` 段配置，现场按调度主站要求整定：

| 配置项 | 默认值 | 现场整定说明 |
|--------|--------|--------------|
| `spi_start` / `spi_end` | 256 / 65535 | 本端可用 SPI 区间；与对端装置协商时从本区间提议 `spi_offer`，须与调度主站侧 SPI 规划不冲突 |
| `replay_window` | 64 | 重放窗口（64 位滑动位图，IPsec 惯例，E9）；**不建议现场调小**（高吞吐乱序场景会误拒），调大需评估位图承载 |
| `key_rotation_interval_ms` | 86400000（24h） | 密钥轮换周期；到期由上层触发 `VerticalEncryptTunnel::rotate(new_keys)` 原位换钥 + 重放窗口清零（E12），业务不中断 |
| `tunnel_policy` | `"sm4-cbc-sm3-hmac"` | 隧道策略：SM4-CBC 加密 + SM3-HMAC 认证（Encrypt-then-MAC）；当前唯一支持策略，字段为后续多策略扩展占位 |
| `ike_timeout_ms` | 5000 | IKE 协商超时兜底；超时未收官按 HandshakeFailed 处理，告警并降级本地自治 |

## 4. 现场互通验收步骤（现场验收项 E10）

> ⚠️ **标注：本节为现场验收项 E10**。蓝图 §2 阻塞条件声明"无对端调度主站测试环境则无法验证互通"——本版本以 Mock 双端回环互通测试替代（TV16~TV23 已覆盖隧道语义），与真实纵向加密装置的互通验收须在**现场部署阶段**按以下 4 步执行并留档。

| 步骤 | 操作 | 验收判据 |
|------|------|---------|
| **① 驱动注入与自检** | 现场实现 `VerticalEncryptDevice` trait 驱动真实加密卡（§1），注入后执行 xmit/poll 自检（回环帧或卡内自测通道） | 驱动 xmit/poll 无 `DeviceError`；Mock 替换为真实驱动后 tunnel 层代码**零改动**编译通过 |
| **② IKE 协商互通** | 与对端纵向加密装置（调度主站侧）执行 SM2 IKE：`initiator_hello` → 对端 `responder_accept` → `initiator_finish` → 双方 `derive_tunnel_keys` | 协商成功建隧（无 HandshakeFailed/CertInvalid）；双方 `TunnelKeys` 一致（以测试接口比对哈希，不比对明文密钥）；`established_count` 计数 +1 |
| **③ 密文隧道互通** | 经已建隧道双向收发业务报文（调度指令/遥测），注入乱序与重复帧验证重放窗口 | 报文双向正确解密（无 TagMismatch/InvalidFrame）；重复帧/超窗帧正确拒绝（`replay_reject_count` 留痕）；窗口内乱序帧正确接收 |
| **④ 性能实测** | 现场流量打流，实测加密吞吐与引入延迟 | **吞吐 ≥ 10Mbps 且加密引入延迟增加 < 5ms**（E10 性能判据）；实测数据留档作为 Phase 2 安全合规出口证据 |

验收通过后：将 4 步记录（含计数器快照、性能实测数据）归档至现场验收报告，作为 36 号文纵向加密合规与 Phase 2 安全合规出口的支撑证据。

---

# 第二部分：合规控制点矩阵（纵向加密部分）

> 依据：36 号文（电力监控系统安全防护"横向隔离、纵向认证"体系）及等保 2.0 三级相关要求；本矩阵覆盖 v0.98.1 纵向加密部分的全部合规控制点，每行给出本实现对应机制与可核查的验收证据。

| 控制点 | 36 号文及等保要求 | 本实现对应机制 | 验收证据 |
|--------|------------------|----------------|----------|
| 纵向边界加密 | 36 号文"纵向认证"：调度主站与厂站间传输必须经纵向加密认证装置加密保护；等保 2.0 三级 8.1.4.2 通信传输保密性 | **SM4-CBC 隧道帧**：帧 `local_spi‖seq:u64‖iv[16]‖SM4-CBC(iv, plaintext)‖SM3-HMAC`；随机 IV 由注入 CsRng 生成（E6，生产接硬件 TRNG，CBC 可预测 IV 不安全已规避）；纵向加密卡以 `VerticalEncryptDevice` trait 适配（E2） | TV16~TV23 隧道收发全链路测试（帧格式解析回读 + 双端加解密一致）；`tunnel_policy = "sm4-cbc-sm3-hmac"` 配置项（configs/vertical-encrypt.toml）；现场验收步骤 ③ 密文互通记录 |
| 消息完整性 | 36 号文纵向传输防篡改；等保 2.0 三级 8.1.4.3 通信传输完整性 | **SM3-HMAC 恒定时间校验**：Encrypt-then-MAC——HMAC 覆盖 `spi‖seq‖iv‖ct` 全帧，先校验后解密（篡改帧不进 CBC 解密路径）；恒定时间比较防时序侧信道；失配即 `Err(TagMismatch)` | TV24~TV26 篡改测试（篡改密文/tag/seq 均 TagMismatch）；`hmac_sm3` / `Sm3Hmac` 纯增量实现（crates/security/crypto/src/sm3/hmac.rs，RFC 2104）；crypto crate 内嵌 hmac 测试（一次性与流式接口一致） |
| 抗重放 | 36 号文防重放攻击（截获重发合法报文不得被接受）；等保 2.0 三级 8.1.4.1 身份鉴别抗重放 | **64 位滑动重放窗口**（E9，IPsec 惯例窗口 64）：u64 seq + 64-bit 滑动位图；`seq` 已收或 `seq <= recv_seq - 64` → `Err(ReplayDetected)`；窗口内乱序帧容忍接收；`replay_reject_count` 计数器留痕可告警 | TV27~TV29 重放测试（同帧二收 ReplayDetected、窗口内乱序正确接收、超窗旧帧 ReplayDetected）；TV37~TV40 管理器路由重放计数验证；`replay_window = 64` 配置项 |
| 密钥协商 | 36 号文纵向认证基于国密算法；等保 2.0 三级 8.1.4.2 密钥安全管理；密钥协商过程须防窃听防抵赖 | **SM2 IKE + PMS 保护**（E3）：PMS[32] 由注入 CsRng 生成 → SM2 加密至对端公钥传输（仅对端私钥可解密，防窃听）→ SM2 签名 `SM3(PMS‖spi)` 防抵赖防篡改 → 双方 `derive_tunnel_keys` SM3 域分离独立派生（"enc"/"auth" 前缀）；**PMS 不出内存明文留存、派生后即弃**；证书 opaque bytes + 复用 `CertVerifier`（E4，PKI v0.32.0 适配器后续注入） | TV6~TV12 IKE 协商测试（双端 PMS 一致、密文篡改 HandshakeFailed、签名篡改 CertInvalid）；TV13~TV15 双端密钥派生一致测试；密钥材料保护代码审查（TunnelKeys 禁 Debug 派生 + Drop 恒定时间清零，Sm3Hmac Drop 清零）；现场验收步骤 ② IKE 互通记录 |
| 密钥轮换 | 36 号文及等保 2.0 三级 8.1.4.2 密钥更新管理：工作密钥须定期更换；证书更新不应中断已有业务连接 | **rotate 原位换钥**（E12）：`VerticalEncryptTunnel::rotate(new_keys)` 原位替换派生密钥 + `send_seq`/`recv_seq`/`replay_bitmap` 清零——不拆建隧道（无 IKE 往返，业务不中断）；旧密钥帧 HMAC 失配自然失效（防跨密钥周期重放）；**隧道持有派生密钥而非证书，证书轮换天然不影响已有连接**（蓝图 §9）；轮换周期 `key_rotation_interval_ms = 86400000`（24h）配置驱动 | TV30~TV32 轮换测试（rotate 后计数清零、新密钥收发正常、旧密钥帧 TagMismatch）；`key_rotation_interval_ms` 配置项（configs/vertical-encrypt.toml）；蓝图 §5 难点"密钥更新与轮换"落地记录（vertical-encrypt-design.md §5.5） |
| 调度主站认证 | 36 号文"纵向认证"：调度主站下发指令须认证来源合法；等保 2.0 三级 8.1.4.1 身份鉴别 | **DispatchToken SM2 验签 + 过期判定**（E8）：`verify_dispatch_auth(token, pk, now_ms)` 对调度主站令牌 payload 做 SM2 验签；**过期判定先于验签**——`now_ms >= expires_ms` → `Expired`（边界等值过期，不验签，防超时指令重放）；三态 `AuthResult { Granted, Denied, Expired }` 完备区分合法/伪造/超时 | TV33~TV36 认证测试（未过期验签 Granted、边界等值 Expired 且证明未验签、异密钥签名 Denied、payload 篡改 Denied）；`DispatchToken` / `AuthResult` 接口契约（vertical-encrypt-design.md §10）；现场验收步骤 ③ 调度指令收发记录 |

---

## 附：合规出口声明

- 本矩阵 6 项控制点全部有**单元测试证据**（TV1~TV40，src 内嵌）+ **配置证据**（configs/vertical-encrypt.toml）+ **设计文档证据**（vertical-encrypt-design.md E1~E12 偏差声明）；
- 现场装置互通（E10）与性能判据（吞吐 ≥10Mbps、延迟增加 <5ms）为**现场验收项**，按第一部分 §4 四步流程执行留档；
- 全部控制点达成后，36 号文纵向加密合规闭环，**Phase 2 安全合规出口条件满足**。
