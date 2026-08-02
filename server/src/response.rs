use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: u16,
    pub data: Option<T>,
    pub msg: String,
}

pub fn ok<T: Serialize>(data: T) -> axum::response::Response {
    (
        StatusCode::OK,
        Json(ApiResponse {
            code: 0,
            data: Some(data),
            msg: "ok".to_string(),
        }),
    )
        .into_response()
}

pub fn created<T: Serialize>(data: T) -> axum::response::Response {
    (
        StatusCode::CREATED,
        Json(ApiResponse {
            code: 0,
            data: Some(data),
            msg: "ok".to_string(),
        }),
    )
        .into_response()
}

pub fn no_content() -> axum::response::Response {
    (
        StatusCode::OK,
        Json(ApiResponse::<()> {
            code: 0,
            data: None,
            msg: "ok".to_string(),
        }),
    )
        .into_response()
}

pub fn err(status: StatusCode, msg: impl ToString) -> axum::response::Response {
    (
        status,
        Json(ApiResponse::<()> {
            code: status.as_u16(),
            data: None,
            msg: msg.to_string(),
        }),
    )
        .into_response()
}

#[track_caller]
pub fn internal_error(e: impl ToString) -> axum::response::Response {
    let msg = e.to_string();
    let caller = std::panic::Location::caller();
    tracing::error!(
        error = %msg,
        caller = %format!("{}:{}", caller.file(), caller.line()),
        "internal server error"
    );
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
}

pub fn not_found(msg: impl ToString) -> axum::response::Response {
    err(StatusCode::NOT_FOUND, msg)
}

pub fn bad_request(msg: impl ToString) -> axum::response::Response {
    err(StatusCode::BAD_REQUEST, msg)
}

pub fn unauthorized(msg: impl ToString) -> axum::response::Response {
    err(StatusCode::UNAUTHORIZED, msg)
}

pub fn forbidden(msg: impl ToString) -> axum::response::Response {
    err(StatusCode::FORBIDDEN, msg)
}

pub fn conflict(msg: impl ToString) -> axum::response::Response {
    err(StatusCode::CONFLICT, msg)
}
