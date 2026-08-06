//! EnerOS 版本化管理 API — RBAC / 幂等性 / 限流 (OBS-010).
//!
//! MASTER §21.13 OBS-010: "versioned management API — RBAC/idempotency/rate limit"。
//! G8 Exit: "管理 API 版本化、角色受限、幂等可重试、限流防爆"。
//!
//! 本 crate 提供：
//! - [`ApiVersion`] — API 版本号（语义化，当前 v1）。
//! - [`Role`] / [`Permission`] — RBAC 角色与权限模型。
//! - [`Request`] / [`Response`] — 版本化请求/响应。
//! - [`IdempotencyStore`] — 幂等键存储（防止重复执行副作用操作）。
//! - [`RateLimiter`] — 滑动窗口限流器。
//! - [`ManagementApi`] — 组合以上组件的请求分发器。
//!
//! # 设计
//!
//! 1. 每个请求携带 `api_version`、`role`、`idempotency_key`（可选）。
//! 2. 先检查版本兼容性 → 限流 → RBAC 权限 → 幂等性 → 执行。
//! 3. 响应包含 `request_id`、`api_version`、状态码、正文。
//! 4. 幂等键在 24 小时窗口内缓存响应，重复请求返回缓存结果。
//! 5. 限流按 role + action 维度，滑动窗口算法。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod auth;
pub mod idempotency;
pub mod rate_limit;
pub mod rbac;
pub mod schema;

use alloc::format;
use alloc::string::String;

pub use auth::{SignedToken, TokenError, TokenVerifier, TOKEN_LEN};
pub use idempotency::{content_hash_key, IdempotencyEntry, IdempotencyError, IdempotencyStore};
pub use rate_limit::{RateLimitConfig, RateLimitError, RateLimiter};
pub use rbac::{Permission, RbacError, RbacPolicy, Role};
pub use schema::{
    ApiAction, ApiError, ApiRequest, ApiResponse, ApiVersion, StatusCode, CURRENT_VERSION,
};

/// 管理 API 核心引擎。
///
/// 组合 RBAC 策略、幂等存储和限流器，按固定顺序检查请求：
/// 版本 → 限流 → 权限 → 幂等 → 执行。
pub struct ManagementApi {
    /// RBAC 策略（角色 → 权限集合）。
    policy: RbacPolicy,
    /// 幂等键存储。
    idempotency: IdempotencyStore,
    /// 限流器。
    rate_limiter: RateLimiter,
    /// 请求计数器（用于生成 request_id）。
    request_counter: u64,
}

impl ManagementApi {
    /// 创建管理 API 实例。
    pub fn new(
        policy: RbacPolicy,
        idempotency: IdempotencyStore,
        rate_limiter: RateLimiter,
    ) -> Self {
        Self {
            policy,
            idempotency,
            rate_limiter,
            request_counter: 0,
        }
    }

    /// 处理一个管理 API 请求。
    ///
    /// 检查顺序：
    /// 1. API 版本兼容性
    /// 2. 限流（按 role + action）
    /// 3. RBAC 权限
    /// 4. 幂等性（如果有 idempotency_key）
    /// 5. 执行操作（由调用方通过 `handler` 提供）
    ///
    /// `handler` 收到 (action, payload) 并返回 (status, body)。
    /// `now_unix` 为当前 Unix 时间戳（秒）。
    pub fn handle<F>(&mut self, request: &ApiRequest, now_unix: u64, handler: F) -> ApiResponse
    where
        F: FnOnce(&ApiAction, &str) -> (StatusCode, String),
    {
        self.request_counter = self.request_counter.wrapping_add(1);
        let request_id = self.request_counter;

        // 1. 版本检查
        if !request.api_version.is_compatible() {
            return ApiResponse::error(
                request_id,
                StatusCode::UnsupportedVersion,
                &format!(
                    "api version {} not supported (current {})",
                    request.api_version.0, CURRENT_VERSION
                ),
            );
        }

        // 2. 限流
        let limit_key = RateLimitKey::new(request.role, request.action);
        match self.rate_limiter.check(&limit_key, now_unix) {
            Ok(()) => {}
            Err(RateLimitError::TooManyRequests { retry_after_secs }) => {
                return ApiResponse::error(
                    request_id,
                    StatusCode::TooManyRequests,
                    &format!("rate limit exceeded, retry after {}s", retry_after_secs),
                );
            }
        }

        // 3. RBAC 权限检查
        if let Err(err) = self.policy.check(request.role, request.action.permission()) {
            return ApiResponse::error(
                request_id,
                StatusCode::Forbidden,
                &format!("rbac denied: {:?}", err),
            );
        }

        // 4. 幂等性检查
        if let Some(key) = &request.idempotency_key {
            if let Some(cached) = self.idempotency.get(key, now_unix) {
                return ApiResponse {
                    request_id,
                    api_version: ApiVersion(CURRENT_VERSION),
                    status: cached.status,
                    body: cached.body.clone(),
                    replayed: true,
                };
            }
        }

        // 5. 执行
        let (status, body) = handler(&request.action, &request.payload);

        // 缓存幂等响应
        if let Some(key) = &request.idempotency_key {
            self.idempotency.put(key, status, &body, now_unix);
        }

        ApiResponse {
            request_id,
            api_version: ApiVersion(CURRENT_VERSION),
            status,
            body,
            replayed: false,
        }
    }
}

/// 限流键（role + action 组合）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateLimitKey {
    pub role: Role,
    pub action: ApiAction,
}

impl RateLimitKey {
    fn new(role: Role, action: ApiAction) -> Self {
        Self { role, action }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request(role: Role, action: ApiAction, payload: &str) -> ApiRequest {
        ApiRequest {
            api_version: ApiVersion(CURRENT_VERSION),
            role,
            action,
            payload: String::from(payload),
            idempotency_key: None,
        }
    }

    fn make_api() -> ManagementApi {
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Admin, Permission::SystemRead);
        policy.grant(Role::Admin, Permission::SystemWrite);
        policy.grant(Role::Operator, Permission::SystemRead);
        let idempotency = IdempotencyStore::new(32);
        let rate_limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 100,
            window_secs: 60,
        });
        ManagementApi::new(policy, idempotency, rate_limiter)
    }

    #[test]
    fn version_incompatible_rejected() {
        let mut api = make_api();
        let req = ApiRequest {
            api_version: ApiVersion(99),
            role: Role::Admin,
            action: ApiAction::StatusQuery,
            payload: String::new(),
            idempotency_key: None,
        };
        let resp = api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("ok")));
        assert_eq!(resp.status, StatusCode::UnsupportedVersion);
    }

    #[test]
    fn rbac_denied_for_unauthorized_role() {
        let mut api = make_api();
        // Viewer only has SystemRead, ConfigSet requires SystemWrite
        let req = make_request(Role::Viewer, ApiAction::ConfigSet, "key=val");
        let resp = api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("ok")));
        assert_eq!(resp.status, StatusCode::Forbidden);
    }

    #[test]
    fn rbac_allowed_for_authorized_role() {
        let mut api = make_api();
        let req = make_request(Role::Admin, ApiAction::StatusQuery, "");
        let resp = api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("ok")));
        assert_eq!(resp.status, StatusCode::Ok);
        assert!(!resp.replayed);
    }

    #[test]
    fn idempotency_returns_cached_response() {
        let mut api = make_api();
        let req = ApiRequest {
            api_version: ApiVersion(CURRENT_VERSION),
            role: Role::Admin,
            action: ApiAction::ConfigSet,
            payload: String::from("key=val"),
            idempotency_key: Some(String::from("abc-123")),
        };
        let resp1 = api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("set")));
        assert_eq!(resp1.status, StatusCode::Ok);
        assert!(!resp1.replayed);

        // 重复请求应返回缓存
        let resp2 = api.handle(&req, 1001, |_, _| {
            (StatusCode::Ok, String::from("SHOULD NOT SEE"))
        });
        assert_eq!(resp2.status, StatusCode::Ok);
        assert!(resp2.replayed);
        assert_eq!(resp2.body, "set");
    }

    #[test]
    fn idempotency_expired_after_window() {
        let mut api = make_api();
        let req = ApiRequest {
            api_version: ApiVersion(CURRENT_VERSION),
            role: Role::Admin,
            action: ApiAction::ConfigSet,
            payload: String::from("key=val"),
            idempotency_key: Some(String::from("abc-123")),
        };
        let resp1 = api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("first")));
        assert!(!resp1.replayed);

        // 25 小时后（超过 24 小时窗口）
        let later = 1000 + 25 * 3600;
        let resp2 = api.handle(&req, later, |_, _| (StatusCode::Ok, String::from("second")));
        assert!(!resp2.replayed);
        assert_eq!(resp2.body, "second");
    }

    #[test]
    fn rate_limit_exceeded_returns_429() {
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Admin, Permission::SystemRead);
        let idempotency = IdempotencyStore::new(32);
        let rate_limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 3,
            window_secs: 60,
        });
        let mut api = ManagementApi::new(policy, idempotency, rate_limiter);

        let req = make_request(Role::Admin, ApiAction::StatusQuery, "");
        for _ in 0..3 {
            let resp = api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("ok")));
            assert_eq!(resp.status, StatusCode::Ok);
        }
        // 第 4 次应被限流
        let resp = api.handle(&req, 1001, |_, _| (StatusCode::Ok, String::from("ok")));
        assert_eq!(resp.status, StatusCode::TooManyRequests);
    }

    #[test]
    fn rate_limit_window_resets() {
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Admin, Permission::SystemRead);
        let idempotency = IdempotencyStore::new(32);
        let rate_limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 2,
            window_secs: 60,
        });
        let mut api = ManagementApi::new(policy, idempotency, rate_limiter);

        let req = make_request(Role::Admin, ApiAction::StatusQuery, "");
        api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("ok")));
        api.handle(&req, 1001, |_, _| (StatusCode::Ok, String::from("ok")));
        let resp = api.handle(&req, 1002, |_, _| (StatusCode::Ok, String::from("ok")));
        assert_eq!(resp.status, StatusCode::TooManyRequests);

        // 61 秒后窗口重置
        let resp = api.handle(&req, 1061, |_, _| (StatusCode::Ok, String::from("ok")));
        assert_eq!(resp.status, StatusCode::Ok);
    }

    #[test]
    fn request_id_increments() {
        let mut api = make_api();
        let req = make_request(Role::Admin, ApiAction::StatusQuery, "");
        let r1 = api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("ok")));
        let r2 = api.handle(&req, 1000, |_, _| (StatusCode::Ok, String::from("ok")));
        assert!(r2.request_id > r1.request_id);
    }
}
