use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Serialize, Deserialize)]
pub struct Article {
    pub title: String,
    #[serde(default)]
    pub excerpt: String,
    #[serde(rename = "contentHtml")]
    pub content_html: String,
    #[serde(default, rename = "facebookSnippet")]
    pub facebook_snippet: String,
    #[serde(default, rename = "imageSuggestions")]
    pub image_suggestions: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub slug: String,
}


#[derive(Debug, Serialize, Deserialize)]
pub struct DraftWithSources {
    pub title: String,
    #[serde(default)]
    pub excerpt: String,
    #[serde(rename = "contentHtml")]
    pub content_html: String,
    pub sources: Vec<SourceRef>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub slug: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SourceRef {
    pub id: String,     // e.g., "S1"
    pub url: String,
    pub title: String,  // human-readable title from the page (optional but useful)
}

#[derive(Debug)]
pub struct SourceCheckResult {
    pub source: crate::models::SourceRef,
    pub status: Option<reqwest::StatusCode>,
    pub error: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SocialAssets {
    #[serde(rename = "facebookSnippet", default)]
    pub facebook_snippet: String,
    #[serde(rename = "imageUrls", default)]
    pub image_urls: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SheetTopic {
    pub row_index: usize,
    pub topic: String,
    pub seed_links: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SocialAssetRow {
    pub sheet_row: usize,
    pub topic: String,
    pub article_title: String,
    pub facebook_snippet: String,
    pub image_urls: Vec<String>,
    pub suggested_tags: Vec<String>,
    pub slug: String,
}


#[derive(Debug, Deserialize)]
pub struct GoogleValuesResponse {
    pub range: Option<String>,
    pub values: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Default)]
pub struct UsageTotals {
    pub input: AtomicU64,
    pub output: AtomicU64,
    pub web_searches: AtomicU64
}

impl UsageTotals {
    pub fn add(&self, input: u64, output: u64) {
        self.input.fetch_add(input, Ordering::Relaxed);
        self.output.fetch_add(output, Ordering::Relaxed);
    }

    pub fn add_web_searches(&self, n: u64) {
        self.web_searches.fetch_add(n, Ordering::Relaxed);
    }
}

#[derive(Copy, Clone, Debug)]
pub enum DraftMode {
    Evergreen,
    News,
}