use reqwest::Client;
use serde_json::Value;
use tokio::time::sleep;
use std::time::Duration;
use reqwest::StatusCode;
use crate::{   
    config::SiteConfig,
    errors::AppError,
    models::Article,
};


pub async fn post_to_wordpress(
    client: &Client,
    cfg: &SiteConfig,
    article: &Article,
    tag_ids: &[u64],
) -> Result<u64, AppError> {
    let url = format!("{}/wp-json/wp/v2/posts", cfg.base_url.trim_end_matches('/'));
    let slug = article.slug.trim();
    if slug.is_empty() {
        return Err(AppError::Http(
            "article slug missing when posting to WordPress".into(),
        ));
    }

    let body = serde_json::json!({
        "title": article.title,
        "slug": slug,
        "content": article.content_html,
        "categories": [21],
        "status": "draft",
        "excerpt": article.excerpt,
        "tags": tag_ids,
        "meta": {
            "description": article.excerpt,
            "_yoast_wpseo_metadesc": article.excerpt
        }
    });

    let mut attempt = 0u32;
    let max_attempts = 4;
    loop {
        attempt += 1;
        let resp = client
            .post(&url)
            .basic_auth(&cfg.username, Some(&cfg.app_password))
            .json(&body)
            .send()
            .await;

        match resp {
            Ok(r) => {
                if r.status().is_success() {
                    let v: Value = r.json().await.map_err(|e| AppError::Http(e.to_string()))?;
                    let id = v["id"].as_u64().unwrap_or(0);
                    if id == 0 {
                        return Err(AppError::Http("WP response missing post ID".into()));
                    }
                    return Ok(id);    // <- ensure we return the Result
                } else if r.status() == StatusCode::TOO_MANY_REQUESTS || r.status().is_server_error()
                {
                    if attempt >= max_attempts {
                        let status = r.status();
                        let body = r.text().await.unwrap_or_default();
                        return Err(AppError::Http(format!(
                            "WP {} after retries: {}",
                            status, body
                        )));
                    }
                } else {
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    return Err(AppError::Http(format!("WP error {}: {}", status, body)));
                }
            }
            Err(e) => {
                if attempt >= max_attempts {
                    return Err(AppError::Http(format!("WP request error: {}", e)));
                }
            }
        }

        let backoff = 2u64.pow(attempt.min(5)) * 250;
        sleep(Duration::from_millis(backoff)).await;
    }
}