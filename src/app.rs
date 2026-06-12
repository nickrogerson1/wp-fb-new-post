use futures::stream::{self, StreamExt};
use reqwest::Client;
use tokio::sync::Mutex;
use clap::Parser;
use std::{
    path::Path, 
    sync::Arc, 
    time::Duration
};

use crate::{
    cli::{Cli, Provider as CliProvider},
    config::load_site_config,
    errors::AppError,
    google::sheets::{load_topics_from_sheet, update_social_assets_in_google_sheet},
    models::{SocialAssetRow, UsageTotals, SheetTopic},
    workflow::process_one_topic,
    utils::{load_price, truncate},
    io_utils::write_social_assets_xlsx,
    providers::{self, ContentProvider},
    config::SiteConfig,
    wp_tags::TagManager,
};

pub async fn run() -> Result<(), AppError> {
    let args = Cli::parse();

    let model = args
        .model
        .clone()
        .unwrap_or_else(|| match args.provider {
            CliProvider::Openai => "gpt-5.2".to_string(),
            CliProvider::Gemini => "gemini-3-pro-preview".to_string(),
        });

    println!("Using provider {:?} with model {}", args.provider, model);

    let client = Client::builder()
        .user_agent("wp-poster/0.1")
        .timeout(Duration::from_secs(2000))
        .build()
        .map_err(|e| AppError::Http(e.to_string()))?;

    let site_cfg = load_site_config()?;

    let provider: Arc<dyn ContentProvider> = match args.provider {
        CliProvider::Openai => {
            let key = std::env::var("OPENAI_API_KEY")
                .map_err(|_| AppError::MissingEnv("OPENAI_API_KEY".into()))?;
            Arc::new(providers::openai::OpenAiProvider::new(
                client.clone(),
                site_cfg.clone(),
                key,
                model.clone(),
            ))
        }
        CliProvider::Gemini => {
            let key = std::env::var("GEMINI_API_KEY")
                .map_err(|_| AppError::MissingEnv("GEMINI_API_KEY".into()))?;
            Arc::new(providers::gemini::GeminiProvider::new(
                client.clone(),
                site_cfg.clone(),
                key,
                model.clone(),
            ))
        }
    };


    let google_sheet_id = std::env::var("GOOGLE_SHEET_ID")
        .map_err(|_| AppError::MissingEnv("GOOGLE_SHEET_ID".into()))?;

    let sheet_name = std::env::var("GOOGLE_SHEET_TAB").unwrap_or_else(|_| "Sheet1".to_string());

    let sheet_topics = load_topics_from_sheet(&client, &google_sheet_id, &sheet_name).await?;
    if sheet_topics.is_empty() {
        println!(
            "No pending prompts found in column C of sheet '{}' ({}).",
            sheet_name, google_sheet_id
        );
        return Ok(());
    }

    ensure_outdir(args.outdir.as_deref())?;

    println!(
        "Processing {} topics for KYI with concurrency {}...",
        sheet_topics.len(),
        args.max_concurrency
    );

    let social_rows = Arc::new(Mutex::new(Vec::<SocialAssetRow>::new()));
    let usage_totals = Arc::new(UsageTotals::default());
    let tag_manager = Arc::new(TagManager::bootstrap(&client, &site_cfg).await?);

   {
        let cache = tag_manager.cache_snapshot().await;

        let mut entries: Vec<_> = cache.iter().collect();
        entries.sort_by_key(|(_, id)| *id);

        println!("WordPress tags loaded ({} total):", entries.len());
        for (name, id) in entries {
            println!("  {} => {}", name, id);
        }
    }

    process_topics(
        provider.clone(),
        &sheet_topics,
        &client,
        &site_cfg,
        &args,
        social_rows.clone(),
        usage_totals.clone(),
        tag_manager,
    )
    .await?;

    finalize_outputs(
        &client,
        &google_sheet_id,
        &sheet_name,
        &social_rows,
        &usage_totals,
        args.provider,
        &model
    )
    .await?;

    Ok(())
}


fn ensure_outdir(dir: Option<&Path>) -> Result<(), AppError> {
    if let Some(path) = dir {
        if !path.exists() {
            std::fs::create_dir_all(path)?;
        }
    }
    Ok(())
}

// async fn process_topics(
//     provider: Arc<dyn ContentProvider>,
//     sheet_topics: &[SheetTopic],
//     client: &Client,
//     site_cfg: &SiteConfig,
//     args: &Cli,
//     social_rows: Arc<Mutex<Vec<SocialAssetRow>>>,
//     usage_totals: Arc<UsageTotals>,
//     tag_manager: Arc<TagManager>,
// ) -> Result<(), AppError> {
//     let results = stream::iter(sheet_topics.iter().enumerate())
//         .map(|(idx, sheet_topic)| {
//             let client = client.clone();
//             let site_cfg = site_cfg.clone();
//             let dry_run = args.dry_run;
//             let outdir = args.outdir.clone();
//             let social_rows = social_rows.clone();
//             let usage_totals = usage_totals.clone();
//             let tag_manager = tag_manager.clone();
//             let sheet_row = sheet_topic.row_index;
//             let topic = sheet_topic.topic.clone();
//             let provider = provider.clone();

//             async move {
//                 match process_one_topic(
//                     provider,
//                     &site_cfg,
//                     &client,
//                     &topic,
//                     sheet_row,
//                     dry_run,
//                     outdir.as_ref(),
//                     social_rows,
//                     usage_totals,
//                     tag_manager,
//                 )
//                 .await
//                 {
//                     Ok(id) => {
//                         println!("[{}/?] OK: {}", idx + 1, topic);
//                         Ok::<_, AppError>(id)
//                     }
//                     Err(e) => {
//                         eprintln!(
//                             "[{}/?] ERROR for topic '{}': {}",
//                             idx + 1,
//                             truncate(&topic, 100),
//                             e
//                         );
//                         Err(e)
//                     }
//                 }
//             }
//         })
//         .buffer_unordered(args.max_concurrency)
//         .collect::<Vec<_>>()
//         .await;

//     let (oks, errs): (Vec<_>, Vec<_>) = results.into_iter().partition(Result::is_ok);
//     println!("Done. Success: {}, Errors: {}", oks.len(), errs.len());

//     if !errs.is_empty() {
//         eprintln!(
//             "{} topic(s) failed. See log above for details, continuing with overall run.",
//             errs.len()
//         );
//     }

//     Ok(())
// }

async fn process_topics(
    provider: Arc<dyn ContentProvider>,
    sheet_topics: &[SheetTopic],
    client: &Client,
    site_cfg: &SiteConfig,
    args: &Cli,
    social_rows: Arc<Mutex<Vec<SocialAssetRow>>>,
    usage_totals: Arc<UsageTotals>,
    tag_manager: Arc<TagManager>,
) -> Result<(), AppError> {
    let draft_mode = if args.news {
        crate::models::DraftMode::News
    } else {
        crate::models::DraftMode::Evergreen
    };

    let results = stream::iter(sheet_topics.iter().enumerate())
        .map(|(idx, sheet_topic)| {
            let client = client.clone();
            let site_cfg = site_cfg.clone();
            let dry_run = args.dry_run;
            let outdir = args.outdir.clone();
            let social_rows = social_rows.clone();
            let usage_totals = usage_totals.clone();
            let tag_manager = tag_manager.clone();
            let sheet_row = sheet_topic.row_index;
            let topic = sheet_topic.topic.clone();
            let seed_links = sheet_topic.seed_links.clone();
            let provider = provider.clone();

            async move {
                match process_one_topic(
                    provider,
                    &site_cfg,
                    &client,
                    &topic,
                    &seed_links,
                    draft_mode,
                    sheet_row,
                    dry_run,
                    outdir.as_ref(),
                    social_rows,
                    usage_totals,
                    tag_manager,
                )
                .await
                {
                    Ok(id) => {
                        println!("[{}/?] OK: {}", idx + 1, topic);
                        Ok::<_, AppError>(id)
                    }
                    Err(e) => {
                        eprintln!(
                            "[{}/?] ERROR for topic '{}': {}",
                            idx + 1,
                            truncate(&topic, 100),
                            e
                        );
                        Err(e)
                    }
                }
            }
        })
        .buffer_unordered(args.max_concurrency)
        .collect::<Vec<_>>()
        .await;

    let (oks, errs): (Vec<_>, Vec<_>) = results.into_iter().partition(Result::is_ok);
    println!("Done. Success: {}, Errors: {}", oks.len(), errs.len());

    if !errs.is_empty() {
        eprintln!(
            "{} topic(s) failed. See log above for details, continuing with overall run.",
            errs.len()
        );
    }

    Ok(())
}

async fn finalize_outputs(
    client: &Client,
    sheet_id: &str,
    sheet_name: &str,
    social_rows: &Arc<Mutex<Vec<SocialAssetRow>>>,
    usage_totals: &Arc<UsageTotals>,
    provider: CliProvider,
    model_name: &str,
) -> Result<(), AppError> {
    let rows_snapshot = {
        let guard = social_rows.lock().await;
        guard.clone()
    };

    if !rows_snapshot.is_empty() {
        if let Err(err) = write_social_assets_xlsx(&rows_snapshot) {
            eprintln!("Failed to write Excel export: {}", err);
        } else {
            println!(
                "Saved {} social asset rows to social_assets.xlsx",
                rows_snapshot.len()
            );
        }

        if let Err(err) =
            update_social_assets_in_google_sheet(
                client, 
                sheet_id, 
                sheet_name, 
                &rows_snapshot,
                model_name
            ).await
        {
            eprintln!("Failed to update Google Sheet: {}", err);
        } else {
            println!(
                "Updated {} rows in the Google Sheet.",
                rows_snapshot.len()
            );
        }
    } else {
        println!("No social assets generated; nothing to push to Google Sheets or Excel.");
    }

     report_usage_and_costs(usage_totals, provider);

    Ok(())
}


fn report_usage_and_costs(usage_totals: &Arc<UsageTotals>, provider: CliProvider) {
    use std::sync::atomic::Ordering;

    let total_input = usage_totals.input.load(Ordering::Relaxed);
    let total_output = usage_totals.output.load(Ordering::Relaxed);
    let grand_total = total_input + total_output;
    let total_searches = usage_totals.web_searches.load(Ordering::Relaxed);

    println!(
        "TOTAL TOKENS for this run – input: {}, output: {}, combined: {}",
        total_input, total_output, grand_total
    );
    println!(
        "TOTAL WEB SEARCHES (sources) for this run: {}",
        total_searches
    );


    let (price_in_var, price_out_var, price_web_var) = match provider {
        CliProvider::Openai => (
            "OPENAI_PRICE_INPUT_PER_1M",
            "OPENAI_PRICE_OUTPUT_PER_1M",
            "OPENAI_WEB_SEARCH_PRICE_PER_1K",
        ),
        CliProvider::Gemini => (
            "GEMINI_PRICE_INPUT_PER_1M",
            "GEMINI_PRICE_OUTPUT_PER_1M",
            "GEMINI_WEB_SEARCH_PRICE_PER_1K",
        ),
    };

    let price_in = load_price(price_in_var);
    let price_out = load_price(price_out_var);
    let price_web = load_price(price_web_var);

    if let (Some(p_in), Some(p_out)) = (price_in, price_out) {
        let cost_input = (total_input as f64 / 1_000_000.0) * p_in;
        let cost_output = (total_output as f64 / 1_000_000.0) * p_out;
        let cost_web = price_web
            .map(|p| (total_searches as f64 / 1_000.0) * p)
            .unwrap_or(0.0);
        let cost_total = cost_input + cost_output + cost_web;

        println!(
            "ESTIMATED COST – input: ${:.4}, output: ${:.4}, web: ${:.4}, combined: ${:.4}",
            cost_input, cost_output, cost_web, cost_total
        );
    } else {
        println!(
            "(Token pricing env vars OPENAI_PRICE_INPUT_PER_1M / OPENAI_PRICE_OUTPUT_PER_1M \
            not set or invalid; skipping cost estimate.)"
        );
    }
}