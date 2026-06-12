use tokio::sync::OnceCell;
use gcp_auth::{provider as gcp_provider, TokenProvider};
use crate::errors::AppError;


static TOKEN_PROVIDER: OnceCell<std::sync::Arc<dyn TokenProvider>> = OnceCell::const_new();

pub async fn google_token_provider() -> Result<&'static std::sync::Arc<dyn TokenProvider>, AppError> {
    TOKEN_PROVIDER
        .get_or_try_init(|| async {
            gcp_provider()
                .await
                .map_err(|e| AppError::GoogleAuth(format!("init provider failed: {e}")))
        })
        .await
}
