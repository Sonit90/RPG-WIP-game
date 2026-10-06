use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use generic_auth_core::AppError;
use serde_json::json;

pub struct ApiError(pub AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Structured field-level validation gets its own shape.
        if let AppError::ValidationFields { message, fields } = &self.0 {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": {
                        "code": "validation",
                        "message": message,
                        "fields": fields,
                    }
                })),
            )
                .into_response();
        }

        let (status, code, message) = match &self.0 {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not_found", self.0.to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", self.0.to_string()),
            AppError::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "unauthorized", self.0.to_string())
            }
            // The inner sentence alone: clients show `message` to players as it is.
            AppError::Validation(m) => (StatusCode::UNPROCESSABLE_ENTITY, "validation", m.clone()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, "conflict", m.clone()),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m.clone()),
            AppError::TooManyRequests(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                self.0.to_string(),
            ),
            AppError::Auth(m) => (StatusCode::UNAUTHORIZED, "auth", m.clone()),
            AppError::EmailNotVerified => (
                StatusCode::FORBIDDEN,
                "email_not_verified",
                self.0.to_string(),
            ),
            AppError::ValidationFields { .. } => unreachable!("handled above"),
            AppError::Storage(_)
            | AppError::Database(_)
            | AppError::Internal(_)
            | AppError::FileProcessing(_) => {
                tracing::error!(error = %self.0, "internal");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal error".into(),
                )
            }
        };
        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_maps_to_409_not_500() {
        let resp =
            ApiError::from(AppError::Conflict("email already exists".into())).into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn database_maps_to_500() {
        let resp = ApiError::from(AppError::Database("boom".into())).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
