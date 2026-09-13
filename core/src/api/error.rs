//! Shared REST error envelope: `{ "error": { "code", "message" } }`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Stable machine-readable error code (snake_case).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    #[allow(dead_code)]
    Conflict,
    #[allow(dead_code)]
    Internal,
    SettingsSaveFailed,
    ProbeFailed,
    DownloadFailed,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Internal => "internal",
            Self::SettingsSaveFailed => "settings_save_failed",
            Self::ProbeFailed => "probe_failed",
            Self::DownloadFailed => "download_failed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// Build a JSON error response with the standard envelope.
pub fn api_error(
    status: StatusCode,
    code: ErrorCode,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(ErrorEnvelope {
            error: ErrorBody {
                code: code.as_str(),
                message: message.into(),
            },
        }),
    )
        .into_response()
}

pub fn bad_request(message: impl Into<String>) -> Response {
    api_error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest, message)
}

pub fn not_found(message: impl Into<String>) -> Response {
    api_error(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
}

#[allow(dead_code)]
pub fn internal(message: impl Into<String>) -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, message)
}
