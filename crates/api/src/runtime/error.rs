use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use tracing::warn;

use crate::model::ErrorResponse;

#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) error: anyhow::Error,
    pub(crate) public_message: Option<String>,
}

impl ApiError {
    pub(crate) fn unauthorized(code: &'static str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code,
            error: anyhow::anyhow!(code),
            public_message: None,
        }
    }

    pub(crate) fn conflict(code: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            error: anyhow::anyhow!(code),
            public_message: None,
        }
    }

    pub(crate) fn conflict_with_message(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            error: anyhow::anyhow!(code),
            public_message: Some(message.into()),
        }
    }

    pub(crate) fn gone(code: &'static str) -> Self {
        Self {
            status: StatusCode::GONE,
            code,
            error: anyhow::anyhow!(code),
            public_message: None,
        }
    }

    pub(crate) fn forbidden(code: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            error: anyhow::anyhow!(code),
            public_message: None,
        }
    }

    pub(crate) fn not_found(code: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            error: anyhow::anyhow!(code),
            public_message: None,
        }
    }

    pub(crate) fn bad_request(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            error: anyhow::anyhow!(code),
            public_message: None,
        }
    }

    pub(crate) fn bad_request_with_message(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            error: anyhow::anyhow!(code),
            public_message: Some(message.into()),
        }
    }

    pub(crate) fn too_many_requests(code: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code,
            error: anyhow::anyhow!(code),
            public_message: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        warn!(
            status = %self.status,
            code = self.code,
            error = %self.error,
            "api request failed"
        );
        (
            self.status,
            Json(ErrorResponse {
                error: self.code.to_string(),
                message: Some(
                    self.public_message
                        .unwrap_or_else(|| default_public_reason(self.status, self.code)),
                ),
                recovery: public_recovery(self.status, self.code).to_string(),
                status: self.status.as_u16(),
            }),
        )
            .into_response()
    }
}

fn default_public_reason(status: StatusCode, code: &str) -> String {
    if status.is_server_error() {
        return "The server could not complete the request safely.".to_string();
    }
    let reason = code.replace('_', " ");
    format!("The request was rejected: {reason}.")
}

pub(crate) fn public_recovery(status: StatusCode, code: &str) -> &'static str {
    if code == "hostname_resolution_timeout" {
        return "Verify resolver reachability and retry; the lookup exceeded its five-second limit.";
    }
    if code == "hostname_resolution_failed" {
        return "Verify the hostname and control-plane DNS configuration before retrying.";
    }
    if code == "hostname_resolution_no_addresses" {
        return "Correct DNS or enter a literal unicast target IP before retrying.";
    }
    if code == "hostname_invalid" {
        return "Enter a valid DNS hostname, or use the literal target-IP field instead.";
    }
    if code.contains("snapshot_stale") || code.contains("confirmation_stale") {
        return "Refresh current state and review the action again before submitting.";
    }
    if code.contains("confirmation") || code.contains("preview_hash") {
        return "Review the current action snapshot before submitting it again.";
    }
    if code.contains("capability") || code.contains("unsupported") {
        return "Inspect the selected VPS agent status and required capability before retrying.";
    }
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            "Correct the submitted values identified by the error code before retrying."
        }
        StatusCode::UNAUTHORIZED => "Authenticate again before retrying this action.",
        StatusCode::FORBIDDEN => {
            "Use an operator scope and privilege assertion that permit this action."
        }
        StatusCode::NOT_FOUND => {
            "Refresh current state and verify that the target still exists within operator scope."
        }
        StatusCode::CONFLICT => {
            "Refresh current state, review the conflict, and submit a new action snapshot."
        }
        StatusCode::GONE => {
            "Refresh current state; the referenced resource or authorization has expired."
        }
        StatusCode::PAYLOAD_TOO_LARGE => "Reduce the request or artifact size and retry.",
        StatusCode::TOO_MANY_REQUESTS => {
            "Wait for the active limit or cooldown to clear before retrying."
        }
        status if status.is_server_error() => {
            "No success is assumed; inspect API logs, refresh current state, and retry only when safe."
        }
        _ => "Refresh current state and review the request before retrying.",
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_server_error",
            error,
            public_message: None,
        }
    }
}

#[cfg(test)]
#[path = "tests_error.rs"]
mod tests;
