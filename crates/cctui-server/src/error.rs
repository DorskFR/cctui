use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use cctui_proto::api::ApiError;

/// One error type for HTTP handlers. It renders as the `{ "error": … }` body
/// every route already returns, and its `From` impls let a handler bubble a
/// `sqlx`/`serde_json` failure with a bare `?` instead of a pasted `map_err`
/// closure. `Status` carries handler-chosen status + message.
#[derive(Debug)]
pub enum AppError {
    Status(StatusCode, String),
    Db(sqlx::Error),
    Json(serde_json::Error),
}

impl AppError {
    pub fn new(code: StatusCode, msg: impl Into<String>) -> Self {
        Self::Status(code, msg.into())
    }

    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::Status(code, _) => *code,
            Self::Db(_) | Self::Json(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Status(_, msg) => msg,
            Self::Db(_) => DB_ERROR,
            Self::Json(_) => "internal error",
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status(code, msg) => write!(f, "{code}: {msg}"),
            Self::Db(e) => write!(f, "db error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<(StatusCode, String)> for AppError {
    fn from((code, msg): (StatusCode, String)) -> Self {
        Self::Status(code, msg)
    }
}

impl From<(StatusCode, &str)> for AppError {
    fn from((code, msg): (StatusCode, &str)) -> Self {
        Self::Status(code, msg.to_owned())
    }
}

pub const DB_ERROR: &str = "database error";

impl AppError {
    /// Log and render as the `(status, { "error": … })` tuple the older
    /// handlers return.
    pub fn into_parts(self) -> (StatusCode, Json<ApiError>) {
        let (code, msg) = match self {
            Self::Status(code, msg) => (code, msg),
            Self::Db(e) => {
                tracing::error!("db error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, DB_ERROR.to_owned())
            }
            Self::Json(e) => {
                tracing::error!("json error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_owned())
            }
        };
        (code, Json(ApiError { error: msg }))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.into_parts().into_response()
    }
}

/// The `{ "error": msg }` tuple the account-family handlers return.
pub fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn db_error_body_is_stable() {
        assert_eq!(super::DB_ERROR, "database error");
    }
}
