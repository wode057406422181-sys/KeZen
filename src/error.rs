use thiserror::Error;

/// Application-level errors
#[derive(Error, Debug)]
pub enum InfiniError {
    #[error("API error: {0}")]
    Api(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Authentication error: no API key configured")]
    NoApiKey,

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
