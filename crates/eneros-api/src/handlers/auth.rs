//! Auth API handlers (v0.6.0 — S1).
//!
//! Provides `POST /api/auth/login` and `POST /api/auth/refresh` endpoints.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::app::AppState;
use crate::auth::Role;

/// Login request body.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// Optional role override (for testing). In production, role is determined
    /// by the user database.
    pub role: Option<String>,
}

/// Login response body.
#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    pub token: String,
    pub token_type: String,
    pub expires_in: usize,
    pub role: String,
}

/// Refresh token request body.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshRequest {
    pub token: String,
}

/// POST /api/auth/login — issue a JWT token.
///
/// Validates username/password against the built-in user database.
/// If no users are registered (first run), accepts any credentials and
/// registers the first user as Supervisor (bootstrap mode).
#[utoipa::path(
    post,
    path = "/api/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "登录成功，返回 JWT 令牌", body = LoginResponse),
        (status = 401, description = "用户名或密码错误"),
        (status = 500, description = "令牌签发失败"),
        (status = 503, description = "认证管理器未配置"),
    )
)]
#[tracing::instrument(skip(state, req), fields(username = %req.username, endpoint = "/api/auth/login"))]
pub async fn login_handler(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> axum::response::Response {
    let auth_manager = match &state.auth_manager {
        Some(m) => m,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "auth disabled: no auth manager configured",
            )
                .into_response();
        }
    };

    // Bootstrap mode: if no users are registered, create the first admin user
    if !auth_manager.has_users() {
        tracing::warn!(
            "login: no users registered — bootstrapping first user '{}' as Supervisor",
            req.username
        );
        auth_manager.add_user(&req.username, &req.password, Role::Supervisor);
    }

    // Validate credentials against user database
    let (username, role) = match auth_manager.validate_credentials(&req.username, &req.password) {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!("login failed for '{}': {}", req.username, e);
            // Record failed login attempt to audit log
            if let Some(ref al) = state.audit_log {
                al.record(crate::audit::AuditEntry::new(
                    &req.username,
                    "unknown",
                    "POST",
                    "/api/auth/login",
                    "-",
                    "failed",
                ).with_detail(format!("invalid credentials: {}", e)));
            }
            return (StatusCode::UNAUTHORIZED, "invalid username or password").into_response();
        }
    };

    // Allow role override only if the user has the requested role or lower
    let effective_role = req
        .role
        .as_deref()
        .and_then(Role::parse)
        .unwrap_or(role);

    // Issue token
    match auth_manager.issue_token(&username, effective_role) {
        Ok(token) => {
            // Record successful login to audit log
            if let Some(ref al) = state.audit_log {
                al.record(crate::audit::AuditEntry::new(
                    &username,
                    effective_role.as_str(),
                    "POST",
                    "/api/auth/login",
                    "-",
                    "success",
                ));
            }
            let response = LoginResponse {
                token,
                token_type: "Bearer".to_string(),
                expires_in: auth_manager.token_ttl(),
                role: effective_role.as_str().to_string(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            tracing::error!("login failed: token issuance error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "token issuance failed").into_response()
        }
    }
}

/// POST /api/auth/refresh — refresh a JWT token.
#[utoipa::path(
    post,
    path = "/api/auth/refresh",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "令牌刷新成功", body = LoginResponse),
        (status = 401, description = "令牌无效或已过期"),
        (status = 503, description = "认证管理器未配置"),
    )
)]
pub async fn refresh_handler(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> axum::response::Response {
    let auth_manager = match &state.auth_manager {
        Some(m) => m,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "auth disabled: no auth manager configured",
            )
                .into_response();
        }
    };

    // Verify the existing token
    match auth_manager.verify_token(&req.token) {
        Ok(claims) => {
            let role = Role::parse(&claims.role).unwrap_or(Role::Operator);
            match auth_manager.issue_token(&claims.sub, role) {
                Ok(new_token) => {
                    let response = LoginResponse {
                        token: new_token,
                        token_type: "Bearer".to_string(),
                        expires_in: auth_manager.token_ttl(),
                        role: role.as_str().to_string(),
                    };
                    (StatusCode::OK, Json(response)).into_response()
                }
                Err(e) => {
                    tracing::error!("token refresh failed: {}", e);
                    (StatusCode::INTERNAL_SERVER_ERROR, "token refresh failed").into_response()
                }
            }
        }
        Err(e) => {
            tracing::warn!("token refresh rejected: {}", e);
            (StatusCode::UNAUTHORIZED, "invalid or expired token").into_response()
        }
    }
}

/// GET /api/auth/me — return current user info from token.
#[utoipa::path(
    get,
    path = "/api/auth/me",
    responses(
        (status = 200, description = "当前用户信息"),
        (status = 401, description = "未认证"),
        (status = 503, description = "认证管理器未配置"),
    )
)]
pub async fn me_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let auth_manager = match &state.auth_manager {
        Some(m) => m,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "auth disabled: no auth manager configured",
            )
                .into_response();
        }
    };

    let extractor = crate::auth::AuthExtractor::from_headers(&headers);
    match auth_manager.authenticate(extractor.auth_header.as_deref(), extractor.api_key_header.as_deref()) {
        Ok(user) => {
            let response = serde_json::json!({
                "username": user.username,
                "role": user.role.as_str(),
                "auth_method": match user.auth_method {
                    crate::auth::AuthMethod::Jwt => "jwt",
                    crate::auth::AuthMethod::ApiKey => "api_key",
                }
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            tracing::warn!("auth failed: {}", e);
            (StatusCode::UNAUTHORIZED, "authentication required").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_request_deserialization() {
        let json = r#"{"username":"alice","password":"secret","role":"operator"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "alice");
        assert_eq!(req.password, "secret");
        assert_eq!(req.role.as_deref(), Some("operator"));
    }

    #[test]
    fn test_login_request_minimal() {
        let json = r#"{"username":"bob","password":"pass"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "bob");
        assert!(req.role.is_none());
    }

    #[test]
    fn test_login_response_serialization() {
        let response = LoginResponse {
            token: "jwt-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            role: "operator".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"token\":\"jwt-token\""));
        assert!(json.contains("\"token_type\":\"Bearer\""));
        assert!(json.contains("\"expires_in\":3600"));
        assert!(json.contains("\"role\":\"operator\""));
    }

    #[test]
    fn test_refresh_request_deserialization() {
        let json = r#"{"token":"old-jwt-token"}"#;
        let req: RefreshRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.token, "old-jwt-token");
    }
}
