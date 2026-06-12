use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SiteConfig {
    pub base_url: String,
    pub username: String,
    pub app_password: String,
    pub site_context: String,
}

pub fn load_site_config() -> Result<SiteConfig, AppError> {
    let base_url = std::env::var("KYI_WP_URL").map_err(|_| AppError::MissingEnv("KYI_WP_URL".into()))?;
    let username = std::env::var("KYI_WP_USER").map_err(|_| AppError::MissingEnv("KYI_WP_USER".into()))?;
    let app_password = std::env::var("KYI_WP_APP_PASSWORD").map_err(|_| AppError::MissingEnv("KYI_WP_APP_PASSWORD".into()))?;
    let site_context = 
        "You are writing for Know Your Instrument (knowyourinstrument.com), an expert site on music topics. 
        Tone: knowledgeable and practical. Audience range tends to be older people interested in music from the 50s to the 90s. 
        Avoid fluff. Provide clear structure with H2/H3, actionable advice, and concise explanations.
        Don't be afraid to make sensational, edgy or thought-provoking claims.".to_string();

     Ok(SiteConfig {
        base_url,
        username,
        app_password,
        site_context,
    })
}

pub fn desired_image_url_limit() -> usize {
    std::env::var("GOOGLE_IMAGE_URL_LIMIT")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(5) // default to 5 if env var missing/invalid
}