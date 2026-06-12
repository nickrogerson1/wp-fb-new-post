use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use tokio::sync::Mutex;
use crate::{   
    config::SiteConfig,
    errors::AppError,
};


#[derive(Debug, Deserialize)]
struct WordPressTag {
    pub id: u64,
    pub name: String,
}

#[derive(Debug)]
pub struct TagManager {
    cache: Mutex<HashMap<String, u64>>,
    creation_lock: Mutex<()>,
}

impl TagManager {
    pub async fn bootstrap(client: &Client, cfg: &SiteConfig) -> Result<Self, AppError> {
        let cache = fetch_existing_tags(client, cfg).await?;
        Ok(Self {
            cache: Mutex::new(cache),
            creation_lock: Mutex::new(()),
        })
    }

    pub async fn ids_for(
        &self,
        client: &Client,
        cfg: &SiteConfig,
        tags: &[String],
    ) -> Result<Vec<u64>, AppError> {
        let mut ids = Vec::new();
        for tag in tags {
            let tag = tag.trim();
            if tag.is_empty() {
                continue;
            }
            ids.push(self.ensure_tag(client, cfg, tag).await?);
        }
        Ok(ids)
    }

    async fn ensure_tag(
        &self,
        client: &Client,
        cfg: &SiteConfig,
        tag: &str,
    ) -> Result<u64, AppError> {
        if let Some(id) = self.lookup(tag).await {
            return Ok(id);
        }

        // Only one task at a time can try to create a missing tag.
        let _guard = self.creation_lock.lock().await;

        if let Some(id) = self.lookup(tag).await {
            return Ok(id);
        }

        let id = create_wordpress_tag(client, cfg, tag).await?;
        let mut cache = self.cache.lock().await;
        cache.insert(tag.to_lowercase(), id);
        Ok(id)
    }

    async fn lookup(&self, tag: &str) -> Option<u64> {
        let cache = self.cache.lock().await;
        cache.get(&tag.to_lowercase()).copied()
    }

    pub async fn cache_snapshot(&self) -> HashMap<String, u64> {
        let cache = self.cache.lock().await;
        cache.clone()
    }
}



async fn fetch_existing_tags(
    client: &Client,
    cfg: &SiteConfig,
) -> Result<HashMap<String, u64>, AppError> {
    let mut tags = HashMap::new();
    let mut page = 1;

    loop {
        let resp = client
            .get(format!(
                "{}/wp-json/wp/v2/tags",
                cfg.base_url.trim_end_matches('/')
            ))
            .query(&[("per_page", "100"), ("page", &page.to_string())])
            .basic_auth(&cfg.username, Some(&cfg.app_password))
            .send()
            .await
            .map_err(|e| AppError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Http(format!(
                "Failed to fetch WP tags ({}): {}",
                status, body
            )));
        }

        let chunk: Vec<WordPressTag> =
            resp.json().await.map_err(|e| AppError::Http(e.to_string()))?;

        if chunk.is_empty() {
            break;
        }

        for tag in &chunk {
            tags.insert(tag.name.to_lowercase(), tag.id);
        }

        if chunk.len() < 100 {
            break;
        }

        page += 1;
    }

    Ok(tags)
}

async fn create_wordpress_tag(
    client: &Client,
    cfg: &SiteConfig,
    tag: &str,
) -> Result<u64, AppError> {
    let resp = client
        .post(format!(
            "{}/wp-json/wp/v2/tags",
            cfg.base_url.trim_end_matches('/')
        ))
        .basic_auth(&cfg.username, Some(&cfg.app_password))
        .json(&serde_json::json!({ "name": tag }))
        .send()
        .await
        .map_err(|e| AppError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Http(format!(
            "Failed to create WP tag '{}' ({}): {}",
            tag, status, body
        )));
    }

    let tag: WordPressTag = resp.json().await.map_err(|e| AppError::Http(e.to_string()))?;
    Ok(tag.id)
}