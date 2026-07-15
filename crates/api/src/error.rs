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
                message: self.public_message,
                status: self.status.as_u16(),
            }),
        )
            .into_response()
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
