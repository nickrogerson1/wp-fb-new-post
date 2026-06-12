use std::sync::Arc;
use async_trait::async_trait;
use reqwest::Client;
use crate::{
    config::SiteConfig,
    errors::AppError,
    models::{Article, DraftMode, DraftWithSources, SocialAssets, UsageTotals},
    providers::ContentProvider,
    gemini::{research_draft, linkify_article_with_chat, generate_social_assets},
};

pub struct GeminiProvider {
    http_client: Client,
    gemini_key: String,
    site_cfg: SiteConfig,
    model: String,
}

impl GeminiProvider {
    pub fn new(http_client: Client, site_cfg: SiteConfig, gemini_key: String, model: String) -> Self {
        Self {
            http_client,
            gemini_key,
            site_cfg,
            model,
        }
    }
}

#[async_trait]
impl ContentProvider for GeminiProvider {
    async fn research_draft(
        &self,
        topic: &str,
        _seed_links: &[String],
        _mode: DraftMode,
        usage_totals: Arc<UsageTotals>,
    ) -> Result<DraftWithSources, AppError> {
        // Gemini implementation currently ignores seed_links/mode.
        // This keeps linkify + social unchanged and satisfies the trait.
        research_draft(
            &self.http_client,
            &self.gemini_key,
            &self.site_cfg,
            &self.model,
            topic,
            usage_totals,
        )
        .await
    }

    async fn linkify(
        &self,
        draft: &DraftWithSources,
        usage_totals: Arc<UsageTotals>,
    ) -> Result<Article, AppError> {
        linkify_article_with_chat(
            &self.http_client,
            &self.gemini_key,
            &self.model,
            draft,
            usage_totals,
        )
        .await
    }

    async fn social_assets(
        &self,
        article: &Article,
        usage_totals: Arc<UsageTotals>,
    ) -> Result<SocialAssets, AppError> {
        generate_social_assets(
            &self.http_client,
            &self.gemini_key,
            &self.model,
            article,
            usage_totals,
        )
        .await
    }
}