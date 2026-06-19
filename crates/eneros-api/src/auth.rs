//! Authentication and authorization (v0.6.0 — S1).
//!
//! Provides JWT-based authentication, API Key authentication, and RBAC
//! permission model with four roles:
//! - **Observer**: read-only access (GET requests)
//! - **Operator**: read + write (GET, POST to non-control endpoints)
//! - **Supervisor**: read + write + control actions (all POST endpoints)
//! - **Emergency**: emergency operations (bypasses some safety checks)
//!
//! ## JWT Format
//!
//! Uses standard JWT (HS256) with claims:
//! ```json
//! {
//!   "sub": "username",
//!   "role": "operator",
//!   "exp": 1718000000,
//!   "iat": 1717996400
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::audit::AuditLog;

type HmacSha256 = Hmac<Sha256>;

/// User roles in the RBAC model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Read-only access
    Observer,
    /// Read + write (non-control endpoints)
    Operator,
    /// Read + write + control actions
    Supervisor,
    /// Emergency operations
    Emergency,
}

impl Role {
    /// Parse a role from a string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "observer" => Some(Self::Observer),
            "operator" => Some(Self::Operator),
            "supervisor" => Some(Self::Supervisor),
            "emergency" => Some(Self::Emergency),
            _ => None,
        }
    }

    /// Check if this role has permission for the given action.
    ///
    /// Permission matrix:
    /// - `read`: all roles
    /// - `write`: Operator, Supervisor, Emergency
    /// - `control`: Supervisor, Emergency
    /// - `emergency`: Emergency only
    pub fn has_permission(&self, action: Permission) -> bool {
        match (self, action) {
            (Role::Observer, Permission::Read) => true,
            (Role::Observer, _) => false,
            (Role::Operator, Permission::Read) | (Role::Operator, Permission::Write) => true,
            (Role::Operator, _) => false,
            (Role::Supervisor, Permission::Read)
            | (Role::Supervisor, Permission::Write)
            | (Role::Supervisor, Permission::Control) => true,
            (Role::Supervisor, Permission::Emergency) => false,
            (Role::Emergency, _) => true,
        }
    }

    /// Get the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Observer => "observer",
            Role::Operator => "operator",
            Role::Supervisor => "supervisor",
            Role::Emergency => "emergency",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Permission actions for RBAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Read data (GET requests)
    Read,
    /// Write data (POST to non-control endpoints)
    Write,
    /// Control actions (POST to /api/actions/*)
    Control,
    /// Emergency operations
    Emergency,
}

/// JWT claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (username)
    pub sub: String,
    /// Role
    pub role: String,
    /// Expiration time (Unix timestamp)
    pub exp: usize,
    /// Issued at (Unix timestamp)
    pub iat: usize,
}

/// Authentication manager handling JWT and API Key auth.
pub struct AuthManager {
    /// JWT signing secret (HMAC-SHA256 key)
    secret: Vec<u8>,
    /// JWT token lifetime in seconds
    token_ttl: usize,
    /// Static API keys: key → (username, role)
    api_keys: RwLock<HashMap<String, (String, Role)>>,
    /// User database: username → (password_hash, role)
    /// Password is stored as SHA-256 hash (not plaintext).
    users: RwLock<HashMap<String, UserRecord>>,
    /// Audit log for recording auth events
    audit_log: Option<Arc<AuditLog>>,
}

/// A user record in the built-in user database.
#[derive(Debug, Clone)]
pub struct UserRecord {
    /// Username
    pub username: String,
    /// SHA-256 hash of the password (hex-encoded)
    pub password_hash: String,
    /// Assigned role
    pub role: Role,
}

impl AuthManager {
    /// Create a new auth manager with the given JWT secret.
    pub fn new(secret: impl Into<Vec<u8>>, token_ttl_secs: usize) -> Self {
        Self {
            secret: secret.into(),
            token_ttl: token_ttl_secs,
            api_keys: RwLock::new(HashMap::new()),
            users: RwLock::new(HashMap::new()),
            audit_log: None,
        }
    }

    /// Attach an audit log to record auth events.
    pub fn with_audit_log(mut self, audit_log: Arc<AuditLog>) -> Self {
        self.audit_log = Some(audit_log);
        self
    }

    /// Register a static API key.
    pub fn add_api_key(&self, key: impl Into<String>, username: impl Into<String>, role: Role) {
        self.api_keys
            .write()
            .insert(key.into(), (username.into(), role));
    }

    /// Register a user with plaintext password (will be hashed internally).
    pub fn add_user(
        &self,
        username: impl Into<String>,
        password: impl AsRef<str>,
        role: Role,
    ) {
        let username = username.into();
        let hash = hash_password(password.as_ref());
        self.users.write().insert(
            username.clone(),
            UserRecord {
                username,
                password_hash: hash,
                role,
            },
        );
    }

    /// Register a user with a pre-hashed password.
    pub fn add_user_with_hash(
        &self,
        username: impl Into<String>,
        password_hash: impl Into<String>,
        role: Role,
    ) {
        let username = username.into();
        self.users.write().insert(
            username.clone(),
            UserRecord {
                username,
                password_hash: password_hash.into(),
                role,
            },
        );
    }

    /// Validate username/password credentials.
    /// Returns (username, role) if valid.
    pub fn validate_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(String, Role), AuthError> {
        let users = self.users.read();
        let record = users
            .get(username)
            .ok_or(AuthError::InvalidCredentials)?;
        let input_hash = hash_password(password);
        if input_hash != record.password_hash {
            return Err(AuthError::InvalidCredentials);
        }
        Ok((record.username.clone(), record.role))
    }

    /// Check if any users are registered.
    pub fn has_users(&self) -> bool {
        !self.users.read().is_empty()
    }

    /// Issue a JWT token for the given username and role.
    pub fn issue_token(&self, username: &str, role: Role) -> Result<String, AuthError> {
        let now = chrono::Utc::now().timestamp() as usize;
        let claims = Claims {
            sub: username.to_string(),
            role: role.as_str().to_string(),
            exp: now + self.token_ttl,
            iat: now,
        };

        let header = r#"{"alg":"HS256","typ":"JWT"}"#;
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?);

        let signing_input = format!("{}.{}", header_b64, payload_b64);

        let mut mac =
            HmacSha256::new_from_slice(&self.secret).map_err(|_| AuthError::InvalidSecret)?;
        mac.update(signing_input.as_bytes());
        let signature = mac.finalize().into_bytes();
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature);

        Ok(format!("{}.{}", signing_input, sig_b64))
    }

    /// Verify a JWT token and return the claims.
    pub fn verify_token(&self, token: &str) -> Result<Claims, AuthError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthError::InvalidTokenFormat);
        }

        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let expected_sig = URL_SAFE_NO_PAD.decode(parts[2])?;

        let mut mac =
            HmacSha256::new_from_slice(&self.secret).map_err(|_| AuthError::InvalidSecret)?;
        mac.update(signing_input.as_bytes());
        mac.verify_slice(&expected_sig)
            .map_err(|_| AuthError::InvalidSignature)?;

        let claims_bytes = URL_SAFE_NO_PAD.decode(parts[1])?;
        let claims: Claims = serde_json::from_slice(&claims_bytes)?;

        // Check expiration
        let now = chrono::Utc::now().timestamp() as usize;
        if claims.exp < now {
            return Err(AuthError::TokenExpired);
        }

        Ok(claims)
    }

    /// Authenticate using an API key.
    /// Returns (username, role) if valid.
    pub fn authenticate_api_key(&self, key: &str) -> Result<(String, Role), AuthError> {
        let api_keys = self.api_keys.read();
        api_keys
            .get(key)
            .cloned()
            .ok_or(AuthError::InvalidApiKey)
    }

    /// Authenticate using a Bearer token (JWT).
    /// Returns (username, role) if valid.
    pub fn authenticate_bearer(&self, token: &str) -> Result<(String, Role), AuthError> {
        let claims = self.verify_token(token)?;
        let role = Role::parse(&claims.role).ok_or(AuthError::InvalidRole)?;
        Ok((claims.sub, role))
    }

    /// Authenticate from raw Authorization header value.
    /// Supports "Bearer <jwt>" and "X-API-Key: <key>".
    pub fn authenticate(
        &self,
        auth_header: Option<&str>,
        api_key_header: Option<&str>,
    ) -> Result<AuthenticatedUser, AuthError> {
        // Try API Key first
        if let Some(key) = api_key_header {
            if let Ok((username, role)) = self.authenticate_api_key(key) {
                return Ok(AuthenticatedUser {
                    username,
                    role,
                    auth_method: AuthMethod::ApiKey,
                });
            }
        }

        // Try Bearer token
        if let Some(header) = auth_header {
            if let Some(token) = header.strip_prefix("Bearer ") {
                if let Ok((username, role)) = self.authenticate_bearer(token) {
                    return Ok(AuthenticatedUser {
                        username,
                        role,
                        auth_method: AuthMethod::Jwt,
                    });
                }
            }
        }

        Err(AuthError::NoValidCredentials)
    }

    /// Check if a user has permission for the given action.
    pub fn check_permission(&self, user: &AuthenticatedUser, action: Permission) -> bool {
        user.role.has_permission(action)
    }

    /// Get the token TTL in seconds.
    pub fn token_ttl(&self) -> usize {
        self.token_ttl
    }
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthManager")
            .field("token_ttl", &self.token_ttl)
            .field("api_key_count", &self.api_keys.read().len())
            .field("has_audit_log", &self.audit_log.is_some())
            .finish()
    }
}

/// An authenticated user.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub username: String,
    pub role: Role,
    pub auth_method: AuthMethod,
}

/// How the user authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    Jwt,
    ApiKey,
}

/// Authentication errors.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid token format: expected 3 parts separated by '.'")]
    InvalidTokenFormat,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("token expired")]
    TokenExpired,
    #[error("invalid API key")]
    InvalidApiKey,
    #[error("invalid role in token")]
    InvalidRole,
    #[error("no valid credentials provided")]
    NoValidCredentials,
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("invalid secret length")]
    InvalidSecret,
    #[error("base64 decode error: {0}")]
    Base64Error(#[from] base64::DecodeError),
    #[error("json error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Axum extractor for the authenticated user.
///
/// This is a lightweight extractor that reads the Authorization and X-API-Key
/// headers. The actual authentication logic is in `AuthManager`.
#[derive(Debug, Clone)]
pub struct AuthExtractor {
    pub auth_header: Option<String>,
    pub api_key_header: Option<String>,
}

impl AuthExtractor {
    /// Extract auth info from axum request headers.
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Self {
        let auth_header = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let api_key_header = headers
            .get("X-API-Key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        Self {
            auth_header,
            api_key_header,
        }
    }
}

/// Determine the required permission for an HTTP method + path.
///
/// - GET → Read
/// - POST to /api/actions/* → Control
/// - POST to /api/emergency/* → Emergency
/// - POST (other) → Write
pub fn required_permission(method: &str, path: &str) -> Permission {
    match method {
        "GET" | "HEAD" | "OPTIONS" => Permission::Read,
        "POST" | "PUT" | "PATCH" | "DELETE" => {
            if path.starts_with("/api/emergency") {
                Permission::Emergency
            } else if path.starts_with("/api/actions") {
                Permission::Control
            } else {
                Permission::Write
            }
        }
        _ => Permission::Read,
    }
}

/// Hash a password using SHA-256 and return the hex-encoded digest.
///
/// This is a simple password hashing function suitable for development and
/// small-scale deployments. For production with many users, consider using
/// bcrypt or argon2 via the `bcrypt` / `argon2` crates.
fn hash_password(password: &str) -> String {
    use sha2::Digest;
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let result = hasher.finalize();
    // Hex-encode the 32-byte digest
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_permissions() {
        // Observer: read only
        assert!(Role::Observer.has_permission(Permission::Read));
        assert!(!Role::Observer.has_permission(Permission::Write));
        assert!(!Role::Observer.has_permission(Permission::Control));
        assert!(!Role::Observer.has_permission(Permission::Emergency));

        // Operator: read + write
        assert!(Role::Operator.has_permission(Permission::Read));
        assert!(Role::Operator.has_permission(Permission::Write));
        assert!(!Role::Operator.has_permission(Permission::Control));
        assert!(!Role::Operator.has_permission(Permission::Emergency));

        // Supervisor: read + write + control
        assert!(Role::Supervisor.has_permission(Permission::Read));
        assert!(Role::Supervisor.has_permission(Permission::Write));
        assert!(Role::Supervisor.has_permission(Permission::Control));
        assert!(!Role::Supervisor.has_permission(Permission::Emergency));

        // Emergency: all
        assert!(Role::Emergency.has_permission(Permission::Read));
        assert!(Role::Emergency.has_permission(Permission::Write));
        assert!(Role::Emergency.has_permission(Permission::Control));
        assert!(Role::Emergency.has_permission(Permission::Emergency));
    }

    #[test]
    fn test_role_from_str() {
        assert_eq!(Role::parse("observer"), Some(Role::Observer));
        assert_eq!(Role::parse("OPERATOR"), Some(Role::Operator));
        assert_eq!(Role::parse("Supervisor"), Some(Role::Supervisor));
        assert_eq!(Role::parse("emergency"), Some(Role::Emergency));
        assert_eq!(Role::parse("admin"), None);
    }

    #[test]
    fn test_role_as_str() {
        assert_eq!(Role::Observer.as_str(), "observer");
        assert_eq!(Role::Operator.as_str(), "operator");
        assert_eq!(Role::Supervisor.as_str(), "supervisor");
        assert_eq!(Role::Emergency.as_str(), "emergency");
    }

    #[test]
    fn test_jwt_issue_and_verify() {
        let manager = AuthManager::new("test-secret-key", 3600);
        let token = manager.issue_token("alice", Role::Operator).unwrap();
        let claims = manager.verify_token(&token).unwrap();

        assert_eq!(claims.sub, "alice");
        assert_eq!(claims.role, "operator");
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn test_jwt_verify_invalid_format() {
        let manager = AuthManager::new("secret", 3600);
        let result = manager.verify_token("invalid.token");
        assert!(matches!(result, Err(AuthError::InvalidTokenFormat)));
    }

    #[test]
    fn test_jwt_verify_invalid_signature() {
        let manager1 = AuthManager::new("secret1", 3600);
        let manager2 = AuthManager::new("secret2", 3600);

        let token = manager1.issue_token("alice", Role::Operator).unwrap();
        let result = manager2.verify_token(&token);
        assert!(matches!(result, Err(AuthError::InvalidSignature)));
    }

    #[test]
    fn test_jwt_verify_expired() {
        let manager = AuthManager::new("secret", 1); // 1 second TTL
        let token = manager.issue_token("alice", Role::Operator).unwrap();

        // Wait for expiration
        std::thread::sleep(std::time::Duration::from_secs(2));

        let result = manager.verify_token(&token);
        assert!(matches!(result, Err(AuthError::TokenExpired)));
    }

    #[test]
    fn test_api_key_auth() {
        let manager = AuthManager::new("secret", 3600);
        manager.add_api_key("my-api-key", "service-account", Role::Supervisor);

        let (username, role) = manager.authenticate_api_key("my-api-key").unwrap();
        assert_eq!(username, "service-account");
        assert_eq!(role, Role::Supervisor);

        let result = manager.authenticate_api_key("wrong-key");
        assert!(matches!(result, Err(AuthError::InvalidApiKey)));
    }

    #[test]
    fn test_authenticate_bearer() {
        let manager = AuthManager::new("secret", 3600);
        let token = manager.issue_token("bob", Role::Emergency).unwrap();

        let (username, role) = manager.authenticate_bearer(&token).unwrap();
        assert_eq!(username, "bob");
        assert_eq!(role, Role::Emergency);
    }

    #[test]
    fn test_authenticate_with_api_key_header() {
        let manager = AuthManager::new("secret", 3600);
        manager.add_api_key("test-key", "service", Role::Observer);

        let user = manager
            .authenticate(None, Some("test-key"))
            .unwrap();
        assert_eq!(user.username, "service");
        assert_eq!(user.role, Role::Observer);
        assert_eq!(user.auth_method, AuthMethod::ApiKey);
    }

    #[test]
    fn test_authenticate_with_bearer_header() {
        let manager = AuthManager::new("secret", 3600);
        let token = manager.issue_token("alice", Role::Operator).unwrap();

        let user = manager
            .authenticate(Some(&format!("Bearer {}", token)), None)
            .unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, Role::Operator);
        assert_eq!(user.auth_method, AuthMethod::Jwt);
    }

    #[test]
    fn test_authenticate_no_credentials() {
        let manager = AuthManager::new("secret", 3600);
        let result = manager.authenticate(None, None);
        assert!(matches!(result, Err(AuthError::NoValidCredentials)));
    }

    #[test]
    fn test_authenticate_invalid_credentials() {
        let manager = AuthManager::new("secret", 3600);
        let result = manager.authenticate(Some("Bearer invalid"), Some("invalid-key"));
        assert!(matches!(result, Err(AuthError::NoValidCredentials)));
    }

    #[test]
    fn test_check_permission() {
        let manager = AuthManager::new("secret", 3600);
        let user = AuthenticatedUser {
            username: "alice".to_string(),
            role: Role::Operator,
            auth_method: AuthMethod::Jwt,
        };

        assert!(manager.check_permission(&user, Permission::Read));
        assert!(manager.check_permission(&user, Permission::Write));
        assert!(!manager.check_permission(&user, Permission::Control));
        assert!(!manager.check_permission(&user, Permission::Emergency));
    }

    #[test]
    fn test_required_permission_get() {
        assert_eq!(required_permission("GET", "/api/agents"), Permission::Read);
        assert_eq!(
            required_permission("HEAD", "/api/agents"),
            Permission::Read
        );
    }

    #[test]
    fn test_required_permission_post() {
        assert_eq!(
            required_permission("POST", "/api/power-flow"),
            Permission::Write
        );
        assert_eq!(
            required_permission("POST", "/api/actions/structured"),
            Permission::Control
        );
        assert_eq!(
            required_permission("POST", "/api/emergency/shed"),
            Permission::Emergency
        );
    }

    #[test]
    fn test_required_permission_put_delete() {
        assert_eq!(
            required_permission("PUT", "/api/devices/1"),
            Permission::Write
        );
        assert_eq!(
            required_permission("DELETE", "/api/agents/1"),
            Permission::Write
        );
    }

    #[test]
    fn test_auth_extractor_from_headers() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer test-token".parse().unwrap(),
        );
        headers.insert("X-API-Key", "test-key".parse().unwrap());

        let extractor = AuthExtractor::from_headers(&headers);
        assert_eq!(extractor.auth_header.as_deref(), Some("Bearer test-token"));
        assert_eq!(extractor.api_key_header.as_deref(), Some("test-key"));
    }

    #[test]
    fn test_auth_extractor_empty_headers() {
        let headers = axum::http::HeaderMap::new();
        let extractor = AuthExtractor::from_headers(&headers);
        assert!(extractor.auth_header.is_none());
        assert!(extractor.api_key_header.is_none());
    }

    #[test]
    fn test_claims_serialization() {
        let claims = Claims {
            sub: "alice".to_string(),
            role: "operator".to_string(),
            exp: 1718000000,
            iat: 1717996400,
        };
        let json = serde_json::to_string(&claims).unwrap();
        let deserialized: Claims = serde_json::from_str(&json).unwrap();
        assert_eq!(claims.sub, deserialized.sub);
        assert_eq!(claims.role, deserialized.role);
    }

    #[test]
    fn test_role_serde() {
        let json = serde_json::to_string(&Role::Supervisor).unwrap();
        assert_eq!(json, "\"supervisor\"");
        let role: Role = serde_json::from_str("\"operator\"").unwrap();
        assert_eq!(role, Role::Operator);
    }
}
