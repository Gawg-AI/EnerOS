# 02 驱动/协议域 全量代码级审查报告（commercial-audit-0811）

- 审查日期：2026-08-11
- 范围：`crates/drivers/` 全部 14 子 crate、`crates/protocols/` 全部 20 子 crate、第三方接缝（`third_party/modbus_peer.py`、smoltcp 使用面）
- 前置对照：蓝图/27（§6-§7 WP-27.2/27.3 D1-D10、§12 WP-27.8、§16 真机填空、§17 M2 互操作判据）、ADR-0028（驱动接缝）、ADR-0040（TSN trait-only）、ADR-0021（agent-bus-dds out-of-v1.0）、evidence/driver-binding/
- 方法：逐文件代码级深读 + grep 全仓调用方清点（二次验证）；host/QEMU 验证判据均不依赖真机

## 0. 汇总

| 统计 | 数量 |
|---|---|
| 总发现 | 30（P0: 0 / P1: 8 / P2: 18 / 观察项: 4） |
| 协议侧（PR-） | 16 |
| 驱动侧（DR-） | 14（含 D2/D5 缺失登记 2 项） |

**P0：无**（诚实结论：未发现数据损坏/安全击穿级缺陷；最严重为 P1 协议正确性与生产缺口）。

**P1 一行清单**：PR-01 IEC104 master 无 S 帧确认/序号校验；PR-02 IEC104 master 无生产传输且整 crate 未装配；PR-03 GOOSE/SV 线格式偏离 IEC 61850；PR-05 IEC104 slave t1 重传循环缺陷；PR-14 IEC104 master adapter 硬编码 127.0.0.1:2404；DR-01 Lfs::write_file 首写必失败；DR-04 cellular PPP 单向协商 + APN 未下发；DR-06 power aarch64 路径硬编码虚构 ADC/GPIO 地址。

---

## 1. 协议侧发现（PR-）

### PR-01｜P1｜BUG｜IEC104 master 不发 S 帧确认、不校验接收序号

- **定位**：`crates/protocols/iec104-master/src/master.rs:338-345`
- **片段**：收到 I 帧后仅处理 ASDU、更新内部状态，未发送 `S(0x01,0x00, rsn, 0)` 确认帧；结构体字段 `pending_acks` 声明后从未被写入/消费（死字段）；无 k 窗口（未确认 I 帧上限）、无 t1/t2 定时器。
- **根因**：链路层状态机只实现了"发送侧轮询"，接收侧确认语义缺失；IEC 60870-5-104 §5.2 要求从站/主站双方按 R(S) 确认。
- **修复方案**：
  - `impl MasterLink { fn ack_received(&mut self, n: u16) }`：收到 I 帧后若 `pending_unacked_rx >= 1` 即发 S 帧；
  - `fn poll(&mut self, now_ms: u64)` 增加 t2（确认时限，默认 10s）与 t3（空闲测试帧 20s）状态；
  - 删除或实现 `pending_acks`（建议实现为 `VecDeque<u16>` 记录已确认序号，用于校验对端 R(S) 合法性）。
- **二次验证**：调用方仅 `adapter.rs`（Iec104MasterAdapter::poll 转发）；adapter 未被 runtime/app/configs 引用（grep 确认 0 命中）→ 波及面限于 crate 内。不变量：序号模 32768 回绕语义需与 apdu.rs 编码一致。次生风险：S 帧发送依赖 transport.send 阻塞语义，mock transport 需同步扩展。
- **host 验证**：`cargo test -p eneros-iec104-master`；新增用例：mock 注入 I 帧 → 断言下一轮 poll 产出 S 帧且 R(S)=N(R)。判据：10 帧连发后 S 帧计数 ≥1，k 超限（12）时暂停发送。
- **与 27 冲突**：WP-27.8（IEC104 master 生产传输）直接相关——链路层不完整加剧该缺口。

### PR-02｜P1｜缺失功能｜IEC104 master 无生产传输，整 crate 未装配

- **定位**：`crates/protocols/iec104-master/src/transport.rs`（`MasterTransport` trait）+ `src/mock.rs`（唯一实现）；全仓 grep `iec104_master|Iec104Master|iec104-master` 于 `crates/runtime/`、`crates/app/`、`configs/` → **0 命中**。
- **根因**：D12 式 trait 解耦已完成，但生产传输（smoltcp/eneros-net 版 `RealMasterTransport`，对照 iec104-slave 的 `transport.rs:76` 实现模式）从未编写；crate 未进装配层。
- **修复方案**：
  - 新建 `src/real.rs`：`pub struct RealMasterTransport { socket: smoltcp TcpSocket handle, now_ms: u64 }`，实现 `MasterTransport::{connect/send/recv/now_ms}`，签名对照 slave 侧 `RealSlaveTransport`；
  - root-task 增加装配条目（config toml 声明远端 IP/端口，禁止硬编码——见 PR-14）。
- **二次验证**：无现有调用方 → 纯新增，零波及；消费方为 adapter.rs（已按 trait 编程）。不变量：connect 失败须 fail-closed（对齐 WP-26.10 模式）。次生风险：无。
- **host 验证**：`cargo test -p eneros-iec104-master --features real-tcp`（新增）+ loopback 集成测试（127.0.0.1:2404 起 iec104-slave 测试桩对拉）。判据：总召 → 遥测回包 PASS。
- **与 27 冲突**：**WP-27.8 本体缺口**（蓝图 §12 已登记"IEC104 master 生产传输"），本次确认仍未落地且整 crate 未接线。

### PR-03｜P1｜缺失功能/互操作｜GOOSE/SV 线格式偏离 IEC 61850

- **定位**：`crates/protocols/iec61850/src/goose_tx.rs`（编码）、`goose_rx.rs:111`、`sv_rx.rs:106`（解码）。
- **片段**：GOOSE/SV PDU 直接以 `0x88b8/0x88ba` EtherType 后接 BER 载荷，**省略 IEC 61850-8-1 要求的 Length(2B)+Reserved(2B) 四字节头**；allData 使用自定义 tag 方言而非标准结构（布尔/位串/整型按标准 [tag] 编码）。
- **根因**：自研双方（tx/rx）闭环互测通过，但未对照 IEC 61850-8-1 §12 线格式；无第三方互操作基准。
- **修复方案**：
  - `goose_tx.rs` 编码处插入 `fn write_pdu_header(out: &mut Vec<u8>, apdu_len: usize)`：写 Length=u16 BE（含自身 4 字节后剩余）、Reserved=0；
  - `goose_rx.rs`/`sv_rx.rs` 解码入口 `fn parse_pdu(buf: &[u8]) -> Result<&[u8], GooseError>` 先剥 4 字节头并校验 Length ≤ buf.len()；
  - allData 按 IEC 61850-8-1 §19 标准类型 tag 重写编解码表。
- **二次验证**：调用方为 iec61850 crate 内订阅/发布路径 + control-gateway（经 trait 消费）；改线格式需同改 tx/rx 与 fixture。不变量：EtherType 与目的组播 MAC 不变。次生风险：既有 fixture 字节全变，需整体重生成；control-gateway 回放证据（evidence/goose-*）失效需重跑。
- **host 验证**：`cargo test -p eneros-iec61850`；补 fixture：第三方抓包样例（可用开源 lib60870/iec61850bean 生成 GOOSE 报文存 `tests/vectors/goose_ref.bin`）→ decode PASS。判据：参考报文解码成功且字段对齐。
- **与 27 冲突**：**与蓝图/27 §17 M2 互操作判据直接冲突**（M2 要求与商用保护装置互操作；当前格式连第三方报文都解不了）。

### PR-04｜P2｜BUG｜0x81 长型 BER 支持不一致

- **定位**：`crates/protocols/iec61850/src/goose_rx.rs:158-177` + `ber_decode.rs:43-62`（不支持 0x81 长型）；`sv_rx.rs:106` 附近解码路径（支持）。
- **根因**：两条解码路径各自演化；长度 >127 的 TLV 在 GOOSE/MMS 路径被拒。
- **修复方案**：统一 `fn ber_length(buf: &[u8], pos: &mut usize) -> Result<usize, BerError>` 支持短型/0x81/0x82，三处调用方收敛到该函数。
- **二次验证**：调用方 = goose_rx、sv_rx、mms 解码；收敛为纯重构+扩展，语义对短型不变。次生风险：mms 侧既有截断安全测试（前轮已核查）需保持 PASS。
- **host 验证**：`cargo test -p eneros-iec61850`；补长度 200 字节 TLV 用例。判据：三种路径同一长型输入均解码成功。
- **与 27 冲突**：无直接 WP；属 PR-03 互操作缺口的子项。

### PR-05｜P1｜BUG｜IEC104 slave t1 超时重传循环缺陷

- **定位**：`crates/protocols/iec104-slave/src/slave.rs:241-258`
- **片段**：t1（发送未确认超时）到期后重发缓冲 I 帧，但重发后未重置 t1 起点/未递增重发计数上限，导致对端长期不确认时以忙循环节奏无限重传。
- **根因**：超时分支缺少 `retries += 1; if retries > MAX { close() }` 与 `t1_start = now` 重置。
- **修复方案**：`fn handle_t1_timeout(&mut self, now_ms: u64)`：重发后 `self.t1_start = now_ms; self.retry_count += 1;`，`retry_count >= 3` 时断开连接（IEC 104 §5.3 建议 k/w 之外以断连保底）。
- **二次验证**：调用方 = slave poll 主循环（唯一）；transport close 已在 trait（`SlaveTransport::close`）。不变量：正常确认路径 retry_count 归零。次生风险：mock 时钟推进用例需按新语义调整断言。
- **host 验证**：`cargo test -p eneros-iec104-slave`；新用例：注入 now_ms 跨过 3×t1 → 断言 close 被调用。判据：无忙循环（重发次数 ≤3）。
- **与 27 冲突**：无（slave 侧已装配生产传输）。

### PR-06｜P2｜BUG｜IEC104 slave APDU encode 长度 u8 截断

- **定位**：`crates/protocols/iec104-slave/src/apdu.rs:145-148`
- **片段**：`let len = payload.len() as u8;` 无 >255 校验，超长 ASDU（如大规模总召响应）静默截断。
- **修复方案**：`fn encode(...) -> Result<Vec<u8>, ApduError>` 前置 `if payload.len() > 249 { return Err(ApduError::TooLong) }`（IEC 104 APDU ≤253 字节）。
- **二次验证**：调用方为 slave.rs 发送路径；错误需向上传播为丢弃该帧+计数。次生风险：总召大表场景可能首次触发该错误 → 需上层分批（既有分帧逻辑存在则无碍）。
- **host 验证**：单测：300 字节 payload → Err。判据：无 panic、错误计数 +1。
- **与 27 冲突**：无。

### PR-07｜P2｜缺失功能｜时钟同步写 RTC 无授权门

- **定位**：`crates/protocols/iec104-slave/src/slave.rs:21-40`（时钟同步 ASDU 处理）与 `:564-581`（写 RTC）。
- **根因**：任意连接方发 C_CS_NA 即可改系统时钟，无鉴权/白名单门。
- **修复方案**：`fn handle_clock_sync(&mut self, ...) -> Result<(), SlaveError>` 增加 `if !self.policy.allow_clock_sync(peer) { return Err(NotAuthorized) }`；policy 由装配层注入（默认拒绝，配置放行）。
- **二次验证**：调用方仅 slave 主循环；policy trait 新增需同步 mock。次生风险：默认拒绝会改变现有测试期望 → 测试显式注入 allow-all policy。
- **host 验证**：单测：未授权 peer 发 C_CS_NA → RTC 未写（断言 rtc mock 调用 0 次）。
- **与 27 冲突**：无；建议并入 29（P0 安全蓝图）的授权矩阵。

### PR-08｜P2｜缺失功能｜双点遥控无 SBO

- **定位**：`crates/protocols/iec104-slave/src/slave.rs:521-558`
- **片段**：单点遥控走 select-before-operate 两段；双点（DPI）分支直接 execute。
- **修复方案**：双点分支复用既有 `fn sbo_check(...)`，缺失的 select 状态机补齐（对照单点实现拷贝适配 2bit 值域）。
- **二次验证**：调用方仅 slave 主循环；既有单点 SBO 测试为模板。次生风险：低。
- **host 验证**：单测：双点 direct-execute 被拒 → 需先 select。判据：无 select 时执行计数 0。
- **与 27 冲突**：无。

### PR-09｜P2｜BUG｜Modbus TCP MBAP 长度 u16 回绕

- **定位**：`crates/protocols/modbus-tcp/src/mbap.rs:35`
- **片段**：`MbapHeader::new` 计算 length = pdu.len()+1，pdu 长度接近 u16::MAX 时回绕。
- **修复方案**：`fn new(...) -> Result<Self, MbapError>`，`pdu.len() > 252`（Modbus 协议上限）直接 Err。
- **二次验证**：调用方为 tcp 服务端收发；错误路径已有。次生风险：无。
- **host 验证**：单测：253+ 字节 PDU → Err。
- **与 27 冲突**：无。

### PR-10｜P2｜BUG｜MQTT CONNECT will payload 长度截断

- **定位**：`crates/protocols/mqtt/src/packet.rs:264-276`
- **片段**：will topic/payload 长度以 `plen as u16` 编码但 body 全量写入 → 长度字段与实际不符，对端解析错位。
- **修复方案**：encode 前 `if topic.len() > u16::MAX || payload.len() > u16::MAX { return Err(PacketError::TooLong) }`。
- **二次验证**：调用方 = client.rs connect；实际 MQTT broker 限制报文 ≤256KB，>65535 的 will 属病态输入 → Err 合理。次生风险：无。
- **host 验证**：单测：70000 字节 will payload → Err。
- **与 27 冲突**：无。

### PR-11｜P2｜命名诚实性｜PlainTls send 静默丢弃返回 Ok

- **定位**：`crates/protocols/mqtt/src/real.rs:205-217`
- **片段**：`PlainTls`（明文传输）的 `send` 在 socket 不可写时丢弃数据仍返回 `Ok(())`。
- **根因**：先例 TlsRecordCodec→FramingRecordCodec 同类问题；名字含 Tls 但无 TLS，且语义吞错。
- **修复方案**：重命名 `PlainTls` → `PlainTcp`；`fn send(&mut self, buf: &[u8]) -> Result<usize, TransportError>` 返回实际写入数，0 写入返回 `Err(WouldBlock)`。
- **二次验证**：调用方 = client.rs 经 MqttTransport trait 消费；trait 签名已返回 Result，改返回值语义需核对 client 重试逻辑（前轮确认 client 无重传——见 PR-12，两处一并修）。次生风险：重命名波及 real.rs 内测试引用。
- **host 验证**：`cargo test -p eneros-mqtt`；grep 全仓 `PlainTls` 归零。判据：send 失败路径测试返回 Err。
- **与 27 冲突**：无；归入命名诚实性专项。

### PR-12｜P2｜缺失功能｜MQTT QoS1/2 无超时重传

- **定位**：`crates/protocols/mqtt/src/client.rs:280-329`
- **片段**：publish QoS1 后等待 PUBACK 无超时；无 DUP 重传逻辑（MQTT 3.1.1 §4.4 要求）。
- **修复方案**：`fn poll(&mut self, now_ms: u64)` 中对 inflight 表每项检查 `now - sent_at > retry_interval(默认 5s)` → 重发（DUP=1，同 packet id）。
- **二次验证**：inflight 表已存在（packet-id 回绕已前轮核查安全）；消费方为 qos_store 持久化路径。不变量：packet-id 不重复分配。次生风险：mock transport 需支持"丢第一帧"用例。
- **host 验证**：单测：mock 丢 PUBACK → 5s 后观测 DUP 重发。判据：重发帧 flags.DUP=1 且 packet id 不变。
- **与 27 冲突**：无。

### PR-13｜P2｜BUG｜MQTT decode 不校验固定头 flags

- **定位**：`crates/protocols/mqtt/src/packet.rs:592-603`
- **片段**：SUBSCRIBE/PUBREL/UNSUBSCRIBE 的固定头低四位必须为 0b0010（MQTT-3.8.1-1 等），decode 未校验。
- **修复方案**：decode 分发表增加 `fn check_fixed_flags(ptype: u8, flags: u8) -> Result<(), PacketError>`，违规 → ProtocolError 断连（规范要求）。
- **二次验证**：调用方 = client 收包循环；断连路径已有。次生风险：对宽容对端更严格，属合规收紧。
- **host 验证**：单测：flags=0 的 SUBSCRIBE → Err。
- **与 27 冲突**：无。

### PR-14｜P1｜硬件耦合｜IEC104 master adapter 硬编码 127.0.0.1:2404

- **定位**：`crates/protocols/iec104-master/src/adapter.rs:89`、`:262-265`
- **片段**：
```rust
let dev = RemoteDevice::new([127, 0, 0, 1], 2404, common_addr, 30_000);
// start():
let dev = RemoteDevice::new([127, 0, 0, 1], 2404, m.common_addr, 30_000);
```
- **根因**：`ProtocolAddress`/`MasterConfig` 无 IP/端口注入字段，联调值写死进生产构造路径。
- **修复方案**：
  - `pub struct MasterConfig { pub peer_ip: [u8; 4], pub peer_port: u16, ... }`；
  - `fn new(config: MasterConfig, transport: Box<dyn MasterTransport>) -> Self` 内改用 `config.peer_ip`；装配层从 configs/*.toml 读取（对齐 binding 表注入模式）。
- **二次验证**：调用方 = adapter 内部 2 处 + 测试（:347 MockTransport 注入处）；外部零调用方（PR-02 已证）→ 波及面仅 crate。不变量：common_addr 语义不变。次生风险：测试需补 config 默认值。
- **host 验证**：`cargo test -p eneros-iec104-master`；grep `127, 0, 0, 1` 于该 crate 仅剩测试段。判据：生产路径无字面量 IP。
- **与 27 冲突**：**WP-27.8 相关**；违反"零硬件依赖/所有板级差异可注入"原则（与驱动侧 DriverBinding 同构要求）。

### PR-15｜P2｜观察｜传输层人工推进时间

- **定位**：`crates/protocols/mqtt/src/real.rs:122`、`crates/protocols/iec61850/src/mms_net.rs:79`
- **片段**：`self.now_ms += 1;` 每次 poll 人工 +1ms 而非读单调时钟。
- **根因**：无时钟注入接缝（与 cellular timeout_tick 同病）。
- **修复方案**：`fn poll(&mut self)` 改从注入的 `trait Clock { fn now_ms(&self) -> u64 }` 读取；host/QEMU 装配层接系统 tick。
- **二次验证**：调用方为各自协议 poll 主循环；trait 注入需同步测试 mock。次生风险：时间语义变化影响超时测试断言（需按真实 ms 校准）。
- **host 验证**：单测以假时钟注入断言超时行为。判据：`now_ms += 1` 全仓归零（生产路径）。
- **与 27 冲突**：对应蓝图/27 §16 时钟/节拍填空项。

### PR-16｜P2｜测试缺口｜tests/interop/ 无 GOOSE/SV 互操作脚本

- **定位**：`tests/interop/`（仅 `iec104.sh mms.sh modbus.sh mqtt.sh interop.sh`）。
- **根因**：GOOSE/SV 无外部 peer 脚本（对照 `third_party/modbus_peer.py` 模式——该脚本本身已核查无问题：CRC16 校验、帧间静默处理正确）。
- **修复方案**：新增 `tests/interop/goose.sh` + `third_party/goose_peer.py`：peer 侧用参考实现（lib60870 或手写标准 BER）发包 → QEMU 侧订阅解码 → 输出 `eneros.goose.v1` 证据；SV 同理。
- **二次验证**：纯新增测试资产，零代码波及。次生风险：依赖 PR-03 修复后格式对齐。
- **host 验证**：`bash tests/interop/goose.sh`（QEMU socket 模式）。判据：PASS 行 ≥5（含 malformed 拒绝）。
- **与 27 冲突**：§17 M2 互操作判据的前置证据缺失。

---

## 2. 驱动侧发现（DR-）

### DR-01｜P1｜BUG｜Lfs::write_file remove 后无法创建文件（首写必失败）

- **定位**：`crates/drivers/fs/src/lfs/filesystem.rs:382-387`
- **片段**：
```rust
fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FsError> {
    let _ = self.remove(path);
    self.write_file_at(path, 0, data)?;   // write_file_at(:149-169) open 仅 .read(true).write(true)，无 .create(true)
    Ok(())
}
```
- **根因**：remove 后文件不存在，`write_file_at` 的 open flags 缺 `create` → littlefs2 返回 NO_SUCH_ENTRY。**对新路径首次写必然失败**。
- **修复方案**：
  - 方案 A（最小）：`write_file_at` 内 open flags 加 `.create(true)`，签名不变；
  - 方案 B（推荐）：write_file 改走 tmp+rename 原子写（对齐 tsdb writer.rs 模式）：`fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FsError>` 先写 `path.tmp`（create=true）再 `rename`，掉电不丢旧数据。
- **二次验证**：调用方清点（grep 全仓 `write_file`）：
  1. `crates/protocols/mqtt/src/qos_store.rs:229`（QoS 持久化，生产）；
  2. `crates/services/device-agent/.../persist.rs:90`（生产）；
  3. `crates/services/cloud-coordinator/.../real.rs:74`（生产）。
  三处均为"文件可能不存在的首写"场景 → 全部受影响。测试盲区根因：fs 泛型测试全走 `fs_trait.rs:191` MockFileSystem（内存 map 版 write_file 正常），掩盖了 Lfs 覆盖版缺陷。波及面：QEMU 上任何经 Lfs 的持久化首次启动即失败。不变量：rename 原子性由 littlefs2 保证。次生风险：方案 B 需临时文件名不与 tsdb `.tmp` 清理逻辑冲突（不同目录则无碍）。
- **host 验证**：`cargo test -p eneros-fs`；新增用例：Lfs（真 littlefs2 + RAM 存储后端）`write_file("a", b"x")` 于空文件系统 → Ok 且可读回。判据：修复前该用例必 FAIL（复现），修复后 PASS。
- **与 27 冲突**：无直接 WP；属交付级阻断 bug（QEMU 冒烟即可暴露）。

### DR-02｜P2｜BUG｜tsdb cleanup_tmp_residue 删除路径错误

- **定位**：`crates/drivers/tsdb/src/db.rs:467`
- **片段**：`let _ = fs.remove(&entry.name);` —— `entry.name` 为基名（如 `1234.tmp`），实际文件在 `metric_path/1234.tmp`；删除必失败且错误被 `let _ =` 吞掉。
- **根因**：列目录返回基名，删除需拼完整路径。
- **修复方案**：`let full = alloc::format!("{}/{}", metric_path, entry.name); let _ = fs.remove(&full);`
- **二次验证**：调用方 = db open 时的启动清理（唯一）；writer.rs 的 tmp+rename 成功后不留残留 → 仅崩溃/掉电场景产生 `.tmp`。波及面：清理失效 → 残留文件永久堆积占 256KB littlefs2 空间（BLOCK_COUNT=64，见观察项）。次生风险：无。
- **host 验证**：单测：预置 `m1/x.tmp` → open → 断言文件消失。判据：修复前 FAIL。
- **与 27 冲突**：无。

### DR-03｜P2｜BUG｜tsdb compact_chunks 子串匹配 vs reader 精确匹配不一致

- **定位**：`crates/drivers/tsdb/src/db.rs:347-350/361-370`（`file_path.contains(&format!("{}/", device.0))`）vs `reader.rs:347-358`（path_matches 精确分段解析）。
- **根因**：两处匹配语义分叉；设备名 `"2"` 的 `contains("2/")` 会误命中 `"12/..."`。
- **修复方案**：把 reader.rs 的 `path_matches` 提升为 crate 内共享函数，compact 改用它：`fn path_matches(path: &str, metric: &MetricPath) -> bool`（已有实现，移动+pub(crate)）。
- **二次验证**：调用方 = compact_chunks 与 reader 查询；合并后语义统一为精确匹配。次生风险：既有 compact 测试若依赖误匹配行为需修正（核查无此依赖）。后果评估：误合并 → 跨设备数据混入 + 源 chunk 被删（数据损坏级，幸而需设备名前缀数字巧合才触发，故定 P2）。
- **host 验证**：单测：设备 `"2"` 与 `"12"` 并存 → compact 后各自独立。判据：无跨对合并。
- **与 27 冲突**：无。

### DR-04｜P1｜缺失功能｜cellular PPP 协商单向 + APN 未下发

- **定位**：`crates/drivers/cellular/src/modem.rs:210-277`（negotiate_ppp/handle_ppp_frame）、`:191-201`（dial）。
- **片段**：`handle_ppp_frame` 只解析对端帧推进本地状态机，**从不发送** LCP Configure-Request/Ack、IPCP 请求（TX 方向整体缺失）；收到 IPCP 带 IP 即直接 `on_auth_success()+on_ipcp_config_ack()`，跳过 PAP/CHAP。`dial()` 中 `self.apn = apn.to_string()` 后直接 `ATD*99#`，**从不发送 `AT+CGDCONT` 配置 APN**。
- **根因**：PPP 状态机按"被动镜像"实现，无 TX 协商报文生成器；APN 存储后无下发步骤。
- **修复方案**：
  - `fn send_lcp_conf_req(&mut self)` / `fn send_ipcp_conf_req(&mut self)`：经 serial 发 HDLC 帧（复用 ppp.rs encode）；
  - `fn dial(&mut self, apn: &str) -> Result<(), ModemError>`：ATD 前先 `AT+CGDCONT=1,"IP","{apn}"` 并等待 OK；
  - 认证：至少实现 PAP（`fn send_pap_auth_req(&mut self, user: &str, pass: &str)`），CHAP 列为后续。
- **二次验证**：调用方 = failover.rs 经 Modem trait；测试用 MockSerial（at_command.rs 编解码已前轮核查无问题）。波及面：cellular 当前未接真实模组（host 全模拟）→ 修复属把模拟语义改成协议正确语义。不变量：failover 状态机转移矩阵不变。次生风险：MockSerial 需支持新 AT 命令的应答脚本。
- **host 验证**：`cargo test -p eneros-cellular`；新用例：dial("cmnet") → 断言 MockSerial 收到 `AT+CGDCONT=1,"IP","cmnet"` 且先于 ATD；注入对端 LCP Conf-Req → 断言我方回 Conf-Ack + 己方 Conf-Req。判据：TX 帧序列符合 RFC 1661 时序。
- **与 27 冲突**：WP-27.3 D10（cellular 设备类）——矩阵已登记 D10，但实现完整度不足，建议补入 §16 填空。

### DR-05｜P2｜缺失功能｜PPP HDLC FCS 固定 0x0000

- **定位**：`crates/drivers/cellular/src/ppp.rs:185-187`
- **片段**：encode 尾部写死 `0x00, 0x00`；decode 不校验 FCS。RFC 1662 要求 CRC-16-CCITT（初始 0xFFFF，校验余数 0xF0B8）。
- **根因**：模块头已自承"Full PPP stack requires hardware verification"，FCS 被占位。
- **修复方案**：`fn fcs16(data: &[u8]) -> u16`（对照 modbus_peer.py 的 CRC16 模式实现 CCITT 变体）；encode 写补码、decode 校验余数。
- **二次验证**：调用方 = modem.rs 编解码两处；与 DR-04 同批改。次生风险：既有测试 fixture 帧尾需重生成。
- **host 验证**：单测：RFC 1662 Appendix 已知向量（"123456789" → 0x906E）。判据：向量 PASS。
- **与 27 冲突**：WP-27.3 D10 相关。

### DR-06｜P1｜硬件耦合｜power detect.rs aarch64 路径硬编码虚构 ADC/GPIO 地址

- **定位**：`crates/drivers/power/src/detect.rs:182/191`
- **片段**：
```rust
#[cfg(target_arch = "aarch64")]
const ADC_BASE: u64 = 0x0906_0000;   // :182
#[cfg(target_arch = "aarch64")]
const GPIO_BASE: u64 = 0x0907_0000;  // :191
```
`is_main_power_ok()` aarch64 版（:229-231）直接 `read_volatile` 上述地址；:72 注释自承"0x0906_0000/0x0907_0000 并非真实 ADC/GPIO（08 号文 B2）"。且 `adc_check_voltage`（:210）把 ADC 原始值直接当毫伏。
- **根因**：v0.216.0 已引入 `PowerRail` 注入接缝且 host 路径（:242-248）已走注入，但 aarch64 生产路径未迁移，形成双轨制。
- **二次验证**：grep 全仓 `is_main_power_ok`：**生产调用方 0**（仅 crate 内测试 :313-385）→ 当前为"地雷"而非活动 bug：一旦被装配接线即读虚构地址（数据损坏级误判掉电）。波及面：power crate 回调链（sequence.rs 已核查为注入式无问题）。不变量：PowerRail 为唯一合法检测入口。次生风险：删除 aarch64 硬编码路径后，现有 cfg(aarch64) 编译路径需统一为 PowerRail。
- **修复方案**：
  - 删除 :180-231 全部 `#[cfg(target_arch = "aarch64")]` 硬编码段；
  - `is_main_power_ok` 统一为单一实现（即现 host 版 :242-248，去掉 cfg gate）；未注入 PowerRail 时 fail-safe 返回缓存状态并 log 警告；
  - 真机 ADC/GPIO 实现作为 `impl PowerRail` 于板级注入（基址来自 FDT，走绑定表 map_mmio，对齐 PL011 模式）。
- **host 验证**：`cargo test -p eneros-power`；`cargo check --target aarch64-unknown-none` 确认无 ADC_BASE/GPIO_BASE 符号。判据：grep `0x0906_0000|0x0907_0000` 全仓归零。
- **与 27 冲突**：违反零硬件依赖总原则；与 crate 自身 v0.216.0 PowerRail 设计冲突；建议列入 §16 真机填空（ADC/GPIO 电源检测的板级实现）。

### DR-07｜P2｜BUG｜rs485 Pl011UartHw divisor u32 溢出 + DE/RE 桩

- **定位**：`crates/drivers/rs485/src/pl011_uart_hw.rs:172`、`:261-266`
- **片段**：`let divisor = (self.clock_hz * 4) / baud_rate;` —— clock_hz 为 u32，≥ ~1.074 GHz 时 `*4` 溢出回绕 → 波特率错。`set_de_re` 仅记录 `de_re_high` 状态位，不操作 GPIO（注释声明"阶段 B 接 hal::HalGpio"）。
- **修复方案**：
  - `let divisor = ((self.clock_hz as u64) * 4 / baud_rate as u64) as u32;`（或 checked_mul + Err）；
  - `fn set_de_re(&mut self, high: bool)` 注入 `trait GpioPin { fn set(&mut self, high: bool) }` 字段，桩实现保留为 `RamGpioPin`（对齐 PowerRail 模式）。
- **二次验证**：调用方 = rs485 driver.rs（已前轮核查 DE/RE 时序逻辑正确）；溢出需高时钟平台才触发（QEMU 24MHz 安全）→ 实际风险低但属确定性 bug。次生风险：GpioPin 注入改构造签名，装配处（driver_probe.rs rs485 条目）需同步。
- **host 验证**：单测：clock_hz=1_200_000_000, baud=115200 → divisor 正确（无回绕）；DE/RE 经 RamGpioPin 断言电平。
- **与 27 冲突**：WP-27.3 D9（CAN/RS485）——DE/RE GPIO 接线属 §16 板级填空，需登记。

### DR-08｜P2｜BUG/缺失｜net MacController TX 路径缓存一致性缺口

- **定位**：`crates/drivers/net/src/mac.rs:304-343`（send）、`:199-227`（init_dma 注释"调用方保证"但无实现）
- **根因**：数据写入堆上 `tx_buffers`，描述符 buffer_addr 指向 DMA 池，两者间无拷贝/cache clean 同步；aarch64 真机 DMA 会读到脏/旧数据。
- **修复方案**：`fn send(&mut self, data: &[u8]) -> Result<(), NetError>` 内：数据拷入 DMA 池缓冲 → `cache_clean_range(addr, len)`（经注入 `trait CacheOps`，host 为空实现）再置描述符；recv 对称加 invalidate。
- **二次验证**：net crate 未入绑定表（D2 缺口）→ 当前零生产调用方；修复与 D2 落地同步做。次生风险：CacheOps trait 新增波及 driver_probe 装配。
- **host 验证**：既有 host 模拟后端测试保持 PASS；新增断言：send 后 DMA 池内容与入参一致。
- **与 27 冲突**：**WP-27.3 D2（virtio-net 缺失，已登记）**——本项是 D2 落地时的必备子项，建议并入 D2 工单。

### DR-09｜P2｜BUG/命名诚实性｜virtio_blk 三处

- **定位**：`crates/drivers/storage/src/virtio_blk.rs:356`、`:682/:709`、`:637-638`
- **片段**：
  1. :356 `capacity` 用 `max_transfer_blocks * 16` 占位，未读 config space 的 capacity 字段；
  2. :682/:709 `block_size > 4096` 无上限校验，后续 `[0u8; 4096]` 栈缓冲越界 panic 面；
  3. :637 注释声称 wait_used 校验 desc id，代码实际无校验。
- **修复方案**：
  1. `fn read_capacity(&mut self) -> u64`：读 config space offset 0（legacy virtio-blk capacity，8 字节 LE）；
  2. `if block_size > 4096 { return Err(StorageError::UnsupportedBlockSize) }`；
  3. wait_used 内核对 `used_elem.id == expected_idx`，不符 → Err；或修正注释为诚实描述。
- **二次验证**：调用方 = storage factory.rs（fail-closed 工厂已核查）；capacity 占位当前只影响上报值，fs 层按块计数不依赖 → 波及低。次生风险：读 config space 需 host 仿真设备模型支持（测试已有 mmio mock，扩展即可）。
- **host 验证**：`cargo test -p eneros-storage`；用例：mock config capacity=1024 → 上报 1024；block_size=8192 → Err。
- **与 27 冲突**：无（D4 块设备已落地，此为精修）。

### DR-10｜P2｜硬件耦合｜WDT 基址 0x09050000 硬编码于装配层

- **定位**：`crates/runtime/root-task/src/main.rs:225` + `panic.rs:19`
- **根因**：watchdog 未走绑定表（binding table 无 sp805 条目），基址字面量直写装配。
- **修复方案**：绑定表增 `arm,sp805` 条目（reg 从 FDT 注入），main.rs/panic.rs 改为查询注册表获取实例；与 D6 工单合并。
- **二次验证**：调用方 = panic 路径喂狗 + 主循环；panic.rs 路径改为经全局 `static WDT: Mutex<Option<&'static dyn WatchdogOps>>`。次生风险：panic 路径不可分配/不可失败（保持现有字面量兜底亦可，但需注释声明）。
- **host 验证**：`cargo test -p root-task`（host 仿真）。判据：grep `0x0905_0000` 仅剩 FDT/测试。
- **与 27 冲突**：**WP-27.3 D6 / §16（已登记）**——本报告确认仍未迁移。

### DR-11｜缺失功能（登记确认）｜D5 RTC 驱动存在但零装配

- **定位**：`crates/drivers/time/src/rtc.rs`（Pl031Rtc 实现完整、Hinnant 算法无溢出，已核查无 bug）；grep 全仓 `Pl031Rtc` 排除 time crate → **0 外部命中**；root-task `main.rs:112 time_init(..., 0)` 以 0 占位。
- **修复方案**：绑定表增 `arm,pl031` 条目，time_init 接 RTC 注入。
- **与 27 冲突**：**WP-27.3 D5（RTC）**——矩阵登记，本次 grep 证实零装配。

### DR-12｜缺失功能（登记确认）｜D2 virtio-net 缺失

- **定位**：`crates/drivers/net/` 有 mac.rs/dma_ring.rs 骨架但无 virtio-net 设备驱动；drv-svc 的 net 服务为 `_start` loop 占位（lib.rs 注释如实声明）。
- **与 27 冲突**：**WP-27.3 D2（已登记缺口）**；与 DR-08 合并为一个工单。

---

## 3. 观察项（P2-观察，不计入修复优先级）

| # | 定位 | 内容 |
|---|---|---|
| O-1 | `watchdog/src/wdt.rs:52` | `timeout_ms * 1000` u32 溢出（>4294s）+ 隐含 1MHz 时钟无注入接缝（§16 时钟填空） |
| O-2 | `fs/src/lfs/storage_adapter.rs:100` | `BLOCK_COUNT=64`（256KB）编译期常量；文档已声明生产需增大，建议改 PlatformCtx 参数 |
| O-3 | `runtime/root-task/src/driver_probe.rs:75/:214/:254` | VIRTIO_REGION_BASE 0x0a00_0000（QEMU 专属）、PL011_CLOCK_HZ 24MHz、SJA1000_CLOCK_HZ 16MHz 硬编码；注释已标注 §16 填空项（合规边界，登记即可） |
| O-4 | `uart/src/lib.rs:156` | BRD 整数截断 FBRD=0，115200@24MHz 误差 ~0.16%（临界可用）；建议实现分数分频 |

## 4. 已核查无发现（诚实记录）

- **驱动**：rs485 driver.rs（DE/RE 时序/帧间隔/统计）、uart_hw.rs trait、pl011 host 仿真测试；cellular failover.rs（转移矩阵+saturating）、at_command.rs（编解码边界）；framework binding.rs（PlatformCtx fail-closed/静态表/ProbeReport 三分类）、registry.rs（权限/索引一致性）；tsdb reader.rs（全边界校验+真 CRC 拒绝损坏）、writer.rs（真 CRC+tmp+rename）、db.rs 主路径；drv-svc virtio-rng（legacy 协商完整/64B 上限/有限轮询）、5/6 服务占位如实声明；uart lib.rs（Pl011Io 注入/溢出如实计数/有界忙等）；time rtc.rs（i64 无溢出/battery-fail→epoch）；storage factory.rs（fail-closed）；can sja1000.rs 全段；power sequence.rs（回调注入式）。
- **绑定表合规**：driver_probe.rs 4 条绑定（pl011 / virtio,mmio / sja1000 / rs485-uart）均走 FDT reg/interrupts 注入，符合 WP-27.2；watchdog/net/power 未入表（见 DR-10/DR-08/DR-06）。
- **第三方接缝**：`third_party/modbus_peer.py` 已核查无问题（CRC16 正确、帧间静默处理、badcrc 分支）；smoltcp 使用面经 MqttTransport/SlaveTransport trait 解耦（ADR-0028 同构模式），无直接硬依赖。
- **协议侧前轮已核查**：MQTT varint/退避/packet-id 回绕、MMS BER 截断安全/握手重试、Modbus RTU master 全段、tsn-time gptp（ADR-0040 trait-only 合规）、control-gateway idempotency、agent-bus-dds codec/signing（ADR-0021 out-of-v1.0 声明在案）。

## 5. 与蓝图/27 冲突汇总

| 发现 | 冲突 WP | 性质 |
|---|---|---|
| PR-02 / PR-14 / PR-01 | WP-27.8（IEC104 master 生产传输） | 已登记缺口，本次确认仍未落地且整 crate 未接线 |
| PR-03 / PR-16 | §17 M2 互操作判据 | **新发现冲突**：线格式偏离标准 + 无互操作脚本，M2 判据当前不可能通过 |
| DR-06 | 零硬件依赖总原则 / §16 | **新发现**：aarch64 虚构地址双轨制，与 PowerRail 自身设计冲突 |
| DR-10 | WP-27.3 D6 / §16 | 已登记，确认未迁移 |
| DR-11 | WP-27.3 D5 | 已登记，grep 证实零装配 |
| DR-12 / DR-08 | WP-27.3 D2 | 已登记，确认缺 cache 一致性子项 |
| DR-04 / DR-05 | WP-27.3 D10 | **新发现**：D10 实现完整度不足（单向 PPP/APN/FCS），建议补登记 §16 |
| DR-07 | WP-27.3 D9 | DE/RE GPIO 接线补登记 §16 |
| PR-15 / O-1 / O-3 | §16 时钟/基址填空 | 已登记类，清点归并 |

## 6. 修复顺序建议（供 29 号蓝图吸收）

1. DR-01（write_file，交付级阻断，~1h）
2. PR-05 + PR-06（slave 正确性，~2h）
3. DR-06（删硬编码，~0.5h）+ DR-02/DR-03（tsdb，~1h）
4. PR-01/PR-02/PR-14（IEC104 master 三连，合并一个工单，~1-2d）
5. PR-03/PR-04/PR-16（GOOSE/SV 格式+互操作，~2-3d，依赖 fixture 重建）
6. 其余 P2 批量清扫。
