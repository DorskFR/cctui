use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TokenRole {
    Agent,
    Admin,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AuthContext {
    pub token: String,
    pub role: TokenRole,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct AuthConfig {
    pub agent_tokens: Vec<String>,
    pub admin_tokens: Vec<String>,
}

#[allow(dead_code)]
impl AuthConfig {
    pub fn validate(&self, token: &str) -> Option<AuthContext> {
        if self.admin_tokens.contains(&token.to_string()) {
            return Some(AuthContext { token: token.to_string(), role: TokenRole::Admin });
        }
        if self.agent_tokens.contains(&token.to_string()) {
            return Some(AuthContext { token: token.to_string(), role: TokenRole::Agent });
        }
        None
    }
}

#[allow(dead_code)]
pub async fn auth_middleware(request: Request, next: Next) -> Result<Response, StatusCode> {
    let auth_config = request
        .extensions()
        .get::<AuthConfig>()
        .cloned()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    let auth_header = request
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let ctx = auth_config.validate(auth_header).ok_or(StatusCode::UNAUTHORIZED)?;

    let mut request = request;
    request.extensions_mut().insert(ctx);

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_agent_token() {
        let config = AuthConfig {
            agent_tokens: vec!["agent-secret".into()],
            admin_tokens: vec!["admin-secret".into()],
        };
        let ctx = config.validate("agent-secret").unwrap();
        assert_eq!(ctx.role, TokenRole::Agent);
    }

    #[test]
    fn validates_admin_token() {
        let config = AuthConfig { agent_tokens: vec![], admin_tokens: vec!["admin-secret".into()] };
        let ctx = config.validate("admin-secret").unwrap();
        assert_eq!(ctx.role, TokenRole::Admin);
    }

    #[test]
    fn rejects_unknown_token() {
        let config = AuthConfig {
            agent_tokens: vec!["agent-secret".into()],
            admin_tokens: vec!["admin-secret".into()],
        };
        assert!(config.validate("wrong").is_none());
    }
}
