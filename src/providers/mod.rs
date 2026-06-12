// pub mod openai;
// pub mod gemini;

// use async_trait::async_trait;
// use std::sync::Arc;

// use crate::{
//     errors::AppError, 
//     models::{DraftWithSources, Article, SocialAssets, UsageTotals},
// };


// #[async_trait]
// pub trait ContentProvider: Send + Sync {
//     async fn research_draft(
//         &self,
//         topic: &str,
//         usage_totals: Arc<UsageTotals>,
//     ) -> Result<DraftWithSources, AppError>;

//     async fn linkify(
//         &self,
//         draft: &DraftWithSources,
//         usage_totals: Arc<UsageTotals>,
//     ) -> Result<Article, AppError>;

//     async fn social_assets(
//         &self,
//         article: &Article,
//         usage_totals: Arc<UsageTotals>,
//     ) -> Result<SocialAssets, AppError>;
// }

pub mod openai;
pub mod gemini;

use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    errors::AppError,
    models::{Article, DraftMode, DraftWithSources, SocialAssets, UsageTotals},
};

#[async_trait]
pub trait ContentProvider: Send + Sync {
    /// Initial drafting/research step.
    /// - `seed_links` comes from sheet column P (may be empty).
    /// - `mode` switches between evergreen vs news prompt styles.
    async fn research_draft(
        &self,
        topic: &str,
        seed_links: &[String],
        mode: DraftMode,
        usage_totals: Arc<UsageTotals>,
    ) -> Result<DraftWithSources, AppError>;

    async fn linkify(
        &self,
        draft: &DraftWithSources,
        usage_totals: Arc<UsageTotals>,
    ) -> Result<Article, AppError>;

    async fn social_assets(
        &self,
        article: &Article,
        usage_totals: Arc<UsageTotals>,
    ) -> Result<SocialAssets, AppError>;
}