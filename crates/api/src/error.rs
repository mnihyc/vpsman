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
}

impl ApiError {
    pub(crate) fn unauthorized(code: &'static str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code,
            error: anyhow::anyhow!(code),
        }
    }

    pub(crate) fn conflict(code: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            error: anyhow::anyhow!(code),
        }
    }

    pub(crate) fn gone(code: &'static str) -> Self {
        Self {
            status: StatusCode::GONE,
            code,
            error: anyhow::anyhow!(code),
        }
    }

    pub(crate) fn forbidden(code: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            error: anyhow::anyhow!(code),
        }
    }

    pub(crate) fn not_found(code: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            error: anyhow::anyhow!(code),
        }
    }

    pub(crate) fn bad_request(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            error: anyhow::anyhow!(code),
        }
    }

    pub(crate) fn too_many_requests(code: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code,
            error: anyhow::anyhow!(code),
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
        }
    }
}
