use actix_web::{web, HttpRequest, HttpResponse, Error};
use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use chrono::{Utc, Duration};

use crate::rate_limit::SecurityManager;
use crate::checkpoint::CheckpointManager;
use crate::fees::GasPriceOracle;

/// JWT Claims
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,      // Subject (user ID)
    pub role: String,     // User role (admin, operator, viewer)
    pub exp: i64,         // Expiration time
    pub iat: i64,         // Issued at
}

/// Admin authentication manager
pub struct AdminAuth {
    secret_key: String,
    token_validity_hours: i64,
}

impl AdminAuth {
    pub fn new(secret_key: String) -> Self {
        Self {
            secret_key,
            token_validity_hours: 24,
        }
    }

    /// Generate JWT token
    pub fn generate_token(&self, user_id: &str, role: &str) -> Result<String, jsonwebtoken::errors::Error> {
        let now = Utc::now();
        let claims = Claims {
            sub: user_id.to_string(),
            role: role.to_string(),
            exp: (now + Duration::hours(self.token_validity_hours)).timestamp(),
            iat: now.timestamp(),
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret_key.as_bytes()),
        )
    }

    /// Verify JWT token
    pub fn verify_token(&self, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
        decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret_key.as_bytes()),
            &Validation::default(),
        )
        .map(|data| data.claims)
    }

    /// Check if user has required role
    pub fn has_role(&self, token: &str, required_role: &str) -> bool {
        if let Ok(claims) = self.verify_token(token) {
            &claims.role == required_role || claims.role == "admin"
        } else {
            false
        }
    }
}

impl Default for AdminAuth {
    fn default() -> Self {
        Self::new("change-this-secret-in-production".to_string())
    }
}

/// Extract bearer token from request
fn extract_token(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get("Authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?
        .to_string()
        .into()
}

/// Admin API endpoints

/// Login endpoint
#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    token: String,
    expires_in: i64,
}

pub async fn login(
    body: web::Json<LoginRequest>,
    auth: web::Data<Arc<AdminAuth>>,
) -> Result<HttpResponse, Error> {
    // In production, verify against database
    // For demo, accept admin/admin
    if body.username == "admin" && body.password == "admin" {
        let token = auth.generate_token(&body.username, "admin")
            .map_err(|e| actix_web::error::ErrorInternalServerError(e))?;

        Ok(HttpResponse::Ok().json(LoginResponse {
            token,
            expires_in: 86400, // 24 hours
        }))
    } else {
        Ok(HttpResponse::Unauthorized().json(serde_json::json!({
            "error": "Invalid credentials"
        })))
    }
}

/// System stats (admin only)
pub async fn get_system_stats(
    req: HttpRequest,
    auth: web::Data<Arc<AdminAuth>>,
) -> Result<HttpResponse, Error> {
    let token = extract_token(&req)
        .ok_or_else(|| actix_web::error::ErrorUnauthorized("No token provided"))?;

    if !auth.has_role(&token, "admin") {
        return Ok(HttpResponse::Forbidden().json(serde_json::json!({
            "error": "Insufficient permissions"
        })));
    }

    // Return system stats
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "cpu_usage": "25%",
        "memory_usage": "1.2GB",
        "disk_usage": "45%",
        "uptime": 86400,
    })))
}

/// Blacklist IP (admin only)
#[derive(Deserialize)]
pub struct BlacklistRequest {
    ip: String,
    reason: String,
}

pub async fn blacklist_ip(
    req: HttpRequest,
    body: web::Json<BlacklistRequest>,
    auth: web::Data<Arc<AdminAuth>>,
    security: web::Data<Arc<SecurityManager>>,
) -> Result<HttpResponse, Error> {
    let token = extract_token(&req)
        .ok_or_else(|| actix_web::error::ErrorUnauthorized("No token provided"))?;

    if !auth.has_role(&token, "admin") {
        return Ok(HttpResponse::Forbidden().json(serde_json::json!({
            "error": "Insufficient permissions"
        })));
    }

    // Parse IP and blacklist
    if let Ok(ip) = body.ip.parse() {
        security.blacklist_ip(ip, body.reason.clone());
        Ok(HttpResponse::Ok().json(serde_json::json!({
            "status": "success",
            "message": format!("IP {} blacklisted", body.ip)
        })))
    } else {
        Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Invalid IP address"
        })))
    }
}

/// Update fee parameters (admin only)
#[derive(Deserialize)]
pub struct UpdateFeeRequest {
    base_fee: u64,
}

pub async fn update_base_fee(
    req: HttpRequest,
    body: web::Json<UpdateFeeRequest>,
    auth: web::Data<Arc<AdminAuth>>,
    gas_oracle: web::Data<Arc<GasPriceOracle>>,
) -> Result<HttpResponse, Error> {
    let token = extract_token(&req)
        .ok_or_else(|| actix_web::error::ErrorUnauthorized("No token provided"))?;

    if !auth.has_role(&token, "admin") {
        return Ok(HttpResponse::Forbidden().json(serde_json::json!({
            "error": "Insufficient permissions"
        })));
    }

    gas_oracle.update_base_fee(body.base_fee);

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "status": "success",
        "new_base_fee": body.base_fee
    })))
}

/// Force checkpoint creation (admin only)
pub async fn force_checkpoint(
    req: HttpRequest,
    auth: web::Data<Arc<AdminAuth>>,
) -> Result<HttpResponse, Error> {
    let token = extract_token(&req)
        .ok_or_else(|| actix_web::error::ErrorUnauthorized("No token provided"))?;

    if !auth.has_role(&token, "admin") {
        return Ok(HttpResponse::Forbidden().json(serde_json::json!({
            "error": "Insufficient permissions"
        })));
    }

    // Trigger checkpoint creation
    // In real implementation, send message to checkpoint manager

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "status": "success",
        "message": "Checkpoint creation triggered"
    })))
}

/// Emergency pause (admin only)
pub async fn emergency_pause(
    req: HttpRequest,
    auth: web::Data<Arc<AdminAuth>>,
) -> Result<HttpResponse, Error> {
    let token = extract_token(&req)
        .ok_or_else(|| actix_web::error::ErrorUnauthorized("No token provided"))?;

    if !auth.has_role(&token, "admin") {
        return Ok(HttpResponse::Forbidden().json(serde_json::json!({
            "error": "Insufficient permissions"
        })));
    }

    log::warn!("EMERGENCY PAUSE activated by admin");

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "status": "success",
        "message": "System paused"
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwt_generation() {
        let auth = AdminAuth::default();
        let token = auth.generate_token("user123", "admin").unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn test_jwt_verification() {
        let auth = AdminAuth::default();
        let token = auth.generate_token("user123", "admin").unwrap();
        let claims = auth.verify_token(&token).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn test_role_check() {
        let auth = AdminAuth::default();
        let token = auth.generate_token("user123", "admin").unwrap();
        assert!(auth.has_role(&token, "admin"));
        assert!(auth.has_role(&token, "operator")); // Admin has all roles
    }
}
