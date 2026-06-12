use std::{path::PathBuf, sync::Arc, time::Instant};
use tokio::sync::Mutex;
use std::fs;
use reqwest::Client;
use crate::{
    config::SiteConfig,
    errors::AppError,
    models::{SocialAssetRow, UsageTotals, DraftMode},
    wordpress::post_to_wordpress,
    io_utils::{sanitize_filename,normalize_tags, short_slug_from_title},
    utils::{fmt_duration, truncate},
    providers::ContentProvider,
    wp_tags::TagManager,
};


// pub async fn process_one_topic(
//     provider: Arc<dyn ContentProvider>,
//     site_cfg: &SiteConfig,
//     client: &Client,
//     topic: &str,
//     sheet_row: usize,
//     dry_run: bool,
//     outdir: Option<&PathBuf>,
//     social_rows: Arc<Mutex<Vec<SocialAssetRow>>>,
//     usage_totals: Arc<UsageTotals>,
//     tag_manager: Arc<TagManager>,
// ) -> Result<String, AppError> {
//     let mut start = Instant::now();

//     let draft = provider.research_draft(topic, usage_totals.clone()).await?;
//     let research_dur = start.elapsed();
//     println!("Draft generated for topic '{}', now linkifying... ", truncate(topic, 80));

//     start = Instant::now();
//     let mut article = provider.linkify(&draft, usage_totals.clone()).await?;
//     let linkify_dur = start.elapsed();
//     println!("Linkification done.");

//     let tags_source: Vec<String> = if article.tags.is_empty() {
//         draft.tags.clone()
//     } else {
//         article.tags.clone()
//     };
//     article.tags = normalize_tags(&tags_source);

//     let slug_source = if article.slug.trim().is_empty() {
//         draft.slug.trim().to_string()
//     } else {
//         article.title.trim().to_string()
//     };
//     let final_slug = short_slug_from_title(&slug_source);
//     article.slug = final_slug.clone();

//     println!("\x1b[31mSlug finalized: '{}'\x1b[0m", article.slug);

//     start = Instant::now();
//     let social_assets = provider.social_assets(&article, usage_totals.clone()).await?;
//     let social_dur = start.elapsed();
//     let total_dur = research_dur + linkify_dur + social_dur;

//     article.facebook_snippet = social_assets.facebook_snippet.clone();
//     article.image_suggestions = social_assets.image_urls.clone();

//     {
//         let mut rows = social_rows.lock().await;
//         rows.push(SocialAssetRow {
//             sheet_row,
//             topic: topic.to_string(),
//             slug: final_slug,
//             article_title: article.title.clone(),
//             suggested_tags: article.tags.clone(),
//             facebook_snippet: article.facebook_snippet.clone(),
//             image_urls: article.image_suggestions.clone(),
//         });
//     }

//     if article.image_suggestions.is_empty() {
//         println!("No image suggestions returned for this topic.\n");
//     }

//     if let Some(dir) = outdir {
//         let fname = sanitize_filename(topic);
//         let path = dir.join(format!("{}.json", fname));
//         let json = serde_json::to_string_pretty(&article)?;
//         fs::write(path, json)?;
//     }

//     if dry_run {
//         println!("DRY RUN: {} => {}", site_cfg.base_url, article.title);
//         println!(
//             "Article Title: {}: \nResearch: {}; Linkification: {}; Social Assets: {}; Total: {}",
//             article.title,
//             fmt_duration(research_dur),
//             fmt_duration(linkify_dur),
//             fmt_duration(social_dur),
//             fmt_duration(total_dur),
//         );
//         return Ok("dry-run".into());
//     }

//     // Resolve tag IDs (creating tags if necessary) before posting.
//     let tag_ids = tag_manager
//         .ids_for(client, site_cfg, &article.tags)
//         .await?;

//     let post_id = post_to_wordpress(client, site_cfg, &article, &tag_ids).await?;

//     println!(
//         "ARTICLE TITLE: {}: Research: {}; Linkification: {}; Social Assets: {}; Total: {}",
//         article.title,
//         fmt_duration(research_dur),
//         fmt_duration(linkify_dur),
//         fmt_duration(social_dur),
//         fmt_duration(total_dur),
//     );

//     Ok(format!("{}", post_id))
// }

pub async fn process_one_topic(
    provider: Arc<dyn ContentProvider>,
    site_cfg: &SiteConfig,
    client: &Client,
    topic: &str,
    seed_links: &[String],
    draft_mode: DraftMode,
    sheet_row: usize,
    dry_run: bool,
    outdir: Option<&PathBuf>,
    social_rows: Arc<Mutex<Vec<SocialAssetRow>>>,
    usage_totals: Arc<UsageTotals>,
    tag_manager: Arc<TagManager>,
) -> Result<String, AppError> {
    let mut start = Instant::now();

    if seed_links.is_empty() {
        eprintln!(
            "\x1b[31m[WARN] Row {}: no seed links found (continuing)\x1b[0m",
            sheet_row
        );
    }

    // ONLY this call changes based on --news and seed links
    let draft = provider
        .research_draft(topic, seed_links, draft_mode, usage_totals.clone())
        .await?;

    let research_dur = start.elapsed();
    println!("Draft generated for topic '{}', now linkifying... ", truncate(topic, 80));

    start = Instant::now();
    let mut article = provider.linkify(&draft, usage_totals.clone()).await?;
    let linkify_dur = start.elapsed();
    println!("Linkification done.");

    let tags_source: Vec<String> = if article.tags.is_empty() {
        draft.tags.clone()
    } else {
        article.tags.clone()
    };
    article.tags = normalize_tags(&tags_source);

    let slug_source = if article.slug.trim().is_empty() {
        draft.slug.trim().to_string()
    } else {
        article.title.trim().to_string()
    };
    let final_slug = short_slug_from_title(&slug_source);
    article.slug = final_slug.clone();

    println!("\x1b[31mSlug finalized: '{}'\x1b[0m", article.slug);

    start = Instant::now();
    let social_assets = provider.social_assets(&article, usage_totals.clone()).await?;
    let social_dur = start.elapsed();
    let total_dur = research_dur + linkify_dur + social_dur;

    article.facebook_snippet = social_assets.facebook_snippet.clone();
    article.image_suggestions = social_assets.image_urls.clone();

    {
        let mut rows = social_rows.lock().await;
        rows.push(SocialAssetRow {
            sheet_row,
            topic: topic.to_string(),
            slug: final_slug,
            article_title: article.title.clone(),
            suggested_tags: article.tags.clone(),
            facebook_snippet: article.facebook_snippet.clone(),
            image_urls: article.image_suggestions.clone(),
        });
    }

    if article.image_suggestions.is_empty() {
        println!("No image suggestions returned for this topic.\n");
    }

    if let Some(dir) = outdir {
        let fname = sanitize_filename(topic);
        let path = dir.join(format!("{}.json", fname));
        let json = serde_json::to_string_pretty(&article)?;
        fs::write(path, json)?;
    }

    if dry_run {
        println!("DRY RUN: {} => {}", site_cfg.base_url, article.title);
        println!(
            "Article Title: {}: \nResearch: {}; Linkification: {}; Social Assets: {}; Total: {}",
            article.title,
            fmt_duration(research_dur),
            fmt_duration(linkify_dur),
            fmt_duration(social_dur),
            fmt_duration(total_dur),
        );
        return Ok("dry-run".into());
    }

    // Resolve tag IDs (creating tags if necessary) before posting.
    let tag_ids = tag_manager
        .ids_for(client, site_cfg, &article.tags)
        .await?;

    let post_id = post_to_wordpress(client, site_cfg, &article, &tag_ids).await?;

    println!(
        "ARTICLE TITLE: {}: Research: {}; Linkification: {}; Social Assets: {}; Total: {}",
        article.title,
        fmt_duration(research_dur),
        fmt_duration(linkify_dur),
        fmt_duration(social_dur),
        fmt_duration(total_dur),
    );

    Ok(format!("{}", post_id))
}