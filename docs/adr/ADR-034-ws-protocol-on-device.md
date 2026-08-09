# ADR-034：W4.5 真 WebSocket 服务——协议在设备侧，传输注入

- 状态：已接受（2026-08-09）
- 关联：蓝图 22 §2.W4.5；ADR-025（W4.1 metrics MQTT）、ADR-030（W4.2 mgmt-api）
- 红线：红线6（目标侧无 std）、红线7（热路径无 unwrap）

## 背景

`DashboardWsServer` 仅有 `MockWsServer`（记录最近事件，非网络）。
验收要求：浏览器连 WebSocket 能看实时数据；MockWsServer 仅 `#[cfg(test)]`。

## 裁决

1. **设备侧实现完整 RFC 6455**（握手 + 帧编解码 + ping/close），no_std + alloc
   零依赖：`ws.rs`（`WsServer` 实现 `DashboardWsServer`）。
2. **传输注入**：`WsServer::new(FnMut(&[u8]) -> Result<(), ()>)`——帧字节写出
   闭包由集成层注入（真 TCP/MQTT 桥），与 W4.1/4.2/4.4 同模式。
3. **SHA-1 + Base64 手写**（RFC 3174/4648），仅用于 `Sec-WebSocket-Accept`
   协议握手，**非安全用途**——EnerOS 安全哈希一律 SM3（crypto 栈），
   此处无新安全依赖。
4. **MockWsServer 移入 `#[cfg(test)]`**（验收第二项），生产代码无 mock。
5. **浏览器兼容性保证**：RFC 6455 官方测试向量（key→accept）+ 字节级
   帧往返测试。

## 理由

- 设备侧无 TCP 栈（无 lwip）；真传输层由 21-B transport 注入闭包承担，
  WS 协议逻辑（握手/帧/JSON 事件）是本模块的「真」——浏览器可直接对话。
- 帧解析 fail-closed：未掩码/RSV 非零/超长（>1MiB）/截断全部拒绝，
  符合红线4。
- 单帧文本消息（FIN=1）广播自包含 JSON，无需消息重组。

## 影响

- 新增 `dashboard/src/ws.rs`（~600 行，纯逻辑 host 可测）；
- `render.rs` 仅保留 trait + 事件类型；
- 浏览器侧验证路径：TCP 桥（未来集成层）→ 握手 → 帧推送；
  host 测试已覆盖握手与帧字节级兼容。
