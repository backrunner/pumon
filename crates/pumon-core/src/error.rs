use thiserror::Error;

#[derive(Debug, Error)]
pub enum PumonError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("runtime resolution error: {0}")]
    Runtime(String),
    #[error("process error: {0}")]
    Process(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type PumonResult<T> = Result<T, PumonError>;
