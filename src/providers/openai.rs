use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;

use crate::{
    config::SiteConfig,
    errors::AppError,
    models::{Article, DraftMode, DraftWithSources, SocialAssets, UsageTotals},
    openai::{generate_social_assets, linkify_article_with_chat, research_draft},
    providers::ContentProvider,
};

pub struct OpenAiProvider {
    http_client: Client,
    openai_key: String,
    site_cfg: SiteConfig,
    model: String,
}

impl OpenAiProvider {
    pub fn new(
        http_client: Client,
        site_cfg: SiteConfig,
        openai_key: String,
        model: String,
    ) -> Self {
        Self {
            http_client,
            openai_key,
            site_cfg,
            model,
        }
    }
}

// #[async_trait]
// impl ContentProvider for OpenAiProvider {
//     async fn research_draft(
//         &self,
//         topic: &str,
//         usage_totals: Arc<UsageTotals>,
//     ) -> Result<DraftWithSources, AppError> {
//         research_draft(
//             &self.http_client,
//             &self.openai_key,
//             &self.site_cfg,
//             &self.model,
//             topic,
//             usage_totals,
//         )
//         .await
//     }

//     async fn linkify(
//         &self,
//         draft: &DraftWithSources,
//         usage_totals: Arc<UsageTotals>,
//     ) -> Result<Article, AppError> {
//         linkify_article_with_chat(
//             &self.http_client,
//             &self.openai_key,
//             &self.model,
//             draft,
//             usage_totals,
//         )
//         .await
//     }

//     async fn social_assets(
//         &self,
//         article: &Article,
//         usage_totals: Arc<UsageTotals>,
//     ) -> Result<SocialAssets, AppError> {
//         generate_social_assets(
//             &self.http_client,
//             &self.openai_key,
//             &self.model,
//             article,
//             usage_totals,
//         )
//         .await
//     }
// }

#[async_trait]
impl ContentProvider for OpenAiProvider {
    async fn research_draft(
        &self,
        topic: &str,
        seed_links: &[String],
        mode: DraftMode,
        usage_totals: Arc<UsageTotals>,
    ) -> Result<DraftWithSources, AppError> {
        research_draft(
            &self.http_client,
            &self.openai_key,
            &self.site_cfg,
            &self.model,
            topic,
            seed_links,
            mode,
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
            &self.openai_key,
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
            &self.openai_key,
            &self.model,
            article,
            usage_totals,
        )
        .await
    }
}