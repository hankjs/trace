//! 错误类型。

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("jsonrpc error {code}: {message}")]
    Rpc {
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    },

    #[error("sse decode error: {0}")]
    Sse(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unexpected result type: {0}")]
    UnexpectedResult(String),
}
