use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Env var missing: {0}")]
    MissingEnv(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("OpenAI returned empty content")]
    EmptyOpenAI,
    #[error("Gemini returned empty content")]
    EmptyGemini,
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Google auth error: {0}")]
    GoogleAuth(String),
    #[error("Google Sheets error: {0}")]
    GoogleSheets(String),
     #[error("Tool error: {0}")]
    Tool(String),  
}