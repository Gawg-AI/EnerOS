//! Server-Sent Events (SSE) 端点 — Dashboard 实时指标推送 (T029-11)。
//!
//! 通过 `GET /api/v1/dashboard/stream` 建立 SSE 连接，订阅 EventBus 上的
//! 实时事件（拓扑变化、潮流数据、Agent 状态、事件告警等），并以 SSE 消息
//! 格式推送到前端。使用 `KeepAlive` 机制在连接空闲时发送心跳注释，避免
//! 代理/负载均衡器因超时关闭连接。
//!
//! 与现有 WebSocket (`/ws`) 并存：SSE 作为主要实时数据源，WebSocket 作为
//! 备份通道，前端 5 秒 REST 轮询作为兜底。

use std::convert::Infallible;

use async_stream::stream;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::Stream;

use crate::app::AppState;

/// `GET /api/v1/dashboard/stream` — SSE 端点，实时推送 Dashboard 指标事件。
///
/// 订阅 `EventBus` 的广播通道，将每个事件序列化为 JSON 并以 SSE `metric`
/// 事件推送到前端。消息格式与 WebSocket EventBus→WS 桥接保持一致，便于
/// 前端复用同一套消息处理逻辑：
///
/// ```text
/// event: metric
/// data: {"type":"event","event_type":"SystemAlarm",...}
///
/// ```
///
/// 当 `AppState.event_bus` 为 `None`（未配置 EventBus）时，流将保持空闲，
/// 由 `KeepAlive` 心跳维持连接，不会产生错误。
#[utoipa::path(
    get,
    path = "/api/v1/dashboard/stream",
    responses(
        (status = 200, description = "SSE 流 — 实时推送 Dashboard 指标事件（event: metric）", content_type = "text/event-stream"),
    ),
    tag = "dashboard",
)]
pub async fn dashboard_stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let event_bus = state.event_bus.clone();

    let stream = stream! {
        if let Some(bus) = event_bus {
            let mut rx = bus.subscribe();
            while let Ok(event) = rx.recv().await {
                // 与 EventBus→WS 桥接使用相同的 JSON 结构，便于前端复用
                let payload = serde_json::json!({
                    "type": "event",
                    "event_type": format!("{:?}", event.event_type),
                    "id": event.id,
                    "timestamp": event.timestamp.to_rfc3339(),
                    "source": event.source,
                    "payload": event.payload,
                });
                let data = match serde_json::to_string(&payload) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("SSE: 事件序列化失败: {}", e);
                        continue;
                    }
                };
                yield Ok(Event::default().event("metric").data(data));
            }
        } else {
            // 未配置 EventBus：流保持空闲，由 KeepAlive 维持连接
            std::future::pending::<()>().await;
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;

    #[test]
    fn test_dashboard_stream_signature() {
        // 验证 handler 函数存在且类型签名正确（编译时检查）
        let _f: fn(State<AppState>) -> _ = dashboard_stream;
    }
}
