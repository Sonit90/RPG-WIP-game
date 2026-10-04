use std::collections::HashMap;
use thiserror::Error;

/// Top-level error used across the workspace. Each crate can `From`-convert
/// its own errors into this, and the `api` crate maps it to HTTP responses.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,

    #[error("forbidden")]
    Forbidden,

    #[error("unauthorized")]
    Unauthorized,

    #[error("validation: {0}")]
    Validation(String),

    #[error("{message}")]
    ValidationFields {
        message: String,
        fields: HashMap<String, Vec<String>>,
    },

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("{0}")]
    TooManyRequests(String),

    #[error("storage: {0}")]
    Storage(String),

    #[error("file processing: {0}")]
    FileProcessing(String),

    #[error("database: {0}")]
    Database(String),

    #[error("auth: {0}")]
    Auth(String),

    #[error("email not confirmed — check your inbox")]
    EmailNotVerified,

    #[error("internal: {0}")]
    Internal(String),
}

impl AppError {
    pub fn internal(msg: impl Into<String>) -> Self {
        AppError::Internal(msg.into())
    }
    pub fn validation(msg: impl Into<String>) -> Self {
        AppError::Validation(msg.into())
    }
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// Convert a `validator::ValidationErrors` into a structured field map
    /// with human-readable messages.
    pub fn from_validation_errors(errs: validator::ValidationErrors) -> Self {
        let mut fields: HashMap<String, Vec<String>> = HashMap::new();
        for (field, kinds) in errs.field_errors() {
            let msgs: Vec<String> = kinds.iter().map(friendly_message).collect();
            fields.insert(field.to_string(), msgs);
        }
        AppError::ValidationFields {
            message: "Validation failed".into(),
            fields,
        }
    }
}

fn friendly_message(e: &validator::ValidationError) -> String {
    if let Some(m) = &e.message {
        return m.to_string();
    }
    match e.code.as_ref() {
        "email" => "Must be a valid email address".into(),
        "url"   => "Must be a valid URL".into(),
        "length" => {
            let min = e.params.get("min").and_then(|v| v.as_u64());
            let max = e.params.get("max").and_then(|v| v.as_u64());
            match (min, max) {
                (Some(a), Some(b)) => format!("Must be between {a} and {b} characters long"),
                (Some(a), None)    => format!("Must be at least {a} characters long"),
                (None,    Some(b)) => format!("Must be at most {b} characters long"),
                _ => "Invalid length".into(),
            }
        }
        "range" => {
            let min = e.params.get("min");
            let max = e.params.get("max");
            match (min, max) {
                (Some(a), Some(b)) => format!("Must be between {a} and {b}"),
                (Some(a), None)    => format!("Must be at least {a}"),
                (None,    Some(b)) => format!("Must be at most {b}"),
                _ => "Out of range".into(),
            }
        }
        "required" => "This field is required".into(),
        other      => format!("Invalid value ({other})"),
    }
}
