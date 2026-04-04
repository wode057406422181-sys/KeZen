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

    #[error(
        "No model configured. Please specify a model via --model, INFINI_MODEL, or config file."
    )]
    NoModel,

    #[error("Stream error: {0}")]
    Stream(String),

    #[allow(dead_code)] // TODO: Use for HTTP 5xx error responses from LLM providers
    #[error("Server error: {0}")]
    Server(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
