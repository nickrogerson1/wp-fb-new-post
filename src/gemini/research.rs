use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use crate::{
    config::SiteConfig,
    errors::AppError,
    gemini::http_request::post_gemini_http,
    models::{DraftWithSources, UsageTotals, SourceRef},
    utils::{collapse_whitespace, extract_json, short_for_spinner, timer_spinner, truncate},
};

use futures::future::join_all;
use reqwest::StatusCode;

#[derive(Debug)]
struct SourceCheckResult {
    source: SourceRef,
    status: Option<StatusCode>,
    error: Option<String>,
    ok: bool,
}

fn build_research_prompts(site_cfg: &SiteConfig, topic: &str) -> (String, String, String) {
    let site_context = &site_cfg.site_context;

    let system = format!(
        r#"Role:
        You are a senior content writer and music expert.
        Write comprehensive, accurate, and helpful articles.
        Don't mention that the article is "sourced" or "well-sourced".
        Don't repeat sections or phrases. Try to keep each section/paragraph well differentiated.

        Output:
        Return ONLY a JSON object with keys: title, excerpt, contentHtml, sources, tags, slug.

        contentHtml must be valid HTML for WordPress, with no <html>, <head>, or <body>.
        contentHtml should be at least 1000 words.
        contentHtml must not exceed 1200 words.
        title <= 100 chars; excerpt <= 160 chars.
        Use H2/H3; short intro and concise conclusion.
        Fun but professional tone tailored to the site.
        Modify the title to make it more eye-catching / engaging.
        Prefer short paragraphs and scannable lists or HTML tables where useful.
        HTML Table headers should be centered.
        Use HTML quotes and blockquotes where appropriate to add effect/emphasis.
        Always attribute quotes and blockquotes to their original source/speaker.
        Paragraphs should be 2-3 sentences in length.
        Be factual. If uncertain, generalize rather than invent specifics.
        Do NOT include <a> tags yet.
        Don't mention the current time or date as these are forever green articles.
        Never use emdashes (—); use regular hyphens (-) only.

        Citations:
        Use [S1], [S2]... in contentHtml.
        Return 'sources' array of {{id, url, title}}.
        Deduplicate; No tracking params.

        When using web_search (or googleSearch) and choosing citations:

        - If there are high-quality, relevant pages available on knowyourinstrument.com, prefer to use them as sources.
        - Only use wikipedia links a maximum of once and only when they are directly relevant to the topic.
        - Place such pages early in the sources list (e.g., [S1], [S2]) when they directly support a claim.
        - ALWAYS try to link to a source when there was an interview, quote, claim, report or opinion to back it up.
        - Do NOT invent URLs on knowyourinstrument.com; only use URLs that actually come from search results.
        - If no suitable page exists on knowyourinstrument.com, fall back to other primary/authoritative sources.
        - Do not use sources multiple times in the same article. Find the most relevant place for each source and use it once.
        - If you need more sources, then keep using search to find them.

        Site context: {site_context}
        "#
    );

    let user1 = format!(r#"Topic: "{topic}""#);

    let user2 = r#"Create the most interesting, engaging, and informative article possible on this topic.
        Feel free to include edgy or provocative angles to hook readers.
        Research the latest credible information using googleSearch.
        Prioritize primary/authoritative sources.
        Return JSON with keys: title, excerpt, contentHtml, sources, tags, slug, webSearches.
        - tags: 5–6 lower-case WordPress tags.
        - slug: dash-separated, <= 8 words, related to the title.
        - webSearches: array of every search query you ran.
        Do not include code fences."#
        .to_string();

    (system, user1, user2)
}

pub async fn research_draft(
    http_client: &reqwest::Client,
    gemini_key: &str,
    site_cfg: &SiteConfig,
    model: &str,
    topic: &str,
    usage_totals: Arc<UsageTotals>,
) -> Result<DraftWithSources, AppError> {
    // Spinner setup via shared utility
    let topic_for_msg = collapse_whitespace(topic);
    let spinner_label = short_for_spinner(&topic_for_msg, 60);
    let spinner_done = timer_spinner("Researching", spinner_label);

    let (system, user1, user2) = build_research_prompts(site_cfg, topic);

    let draft_schema = json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "excerpt": { "type": "string" },
            "contentHtml": { "type": "string" },
            "sources": {
                "type": "array",
                "minItems": 8,   
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "url": { "type": "string", "format": "uri" },
                        "title": { "type": "string" }
                    },
                    "required": ["id", "url", "title"]
                }
            },
            "tags": { "type": "array", "items": { "type": "string" } },
            "slug": { "type": "string" },
            "webSearches": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["title","excerpt","contentHtml","sources","tags","slug"]
    });

    let body = json!({
        "system_instruction": { "parts": [{ "text": system }] },
        "contents": [
            { "role": "user", "parts": [{ "text": user1 }] },
            { "role": "user", "parts": [{ "text": user2 }] }
        ],
        "tools": [
        {
            "functionDeclarations": [
            {
                "name": "httpHead",
                "description": "Fetch a URL (HEAD or GET) and report the HTTP status.",
                "parameters": {
                "type": "object",
                "properties": {
                    "url": {
                    "type": "string",
                    "description": "Fully-qualified URL to verify",
                    "format": "uri"
                    },
                    "method": {
                    "type": "string",
                    "enum": ["HEAD", "GET"],
                    "description": "HTTP method to use (default HEAD)"
                    }
                },
                "required": ["url"]
                }
            }
            ]
        },
        { "googleSearch": {} }
        ],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseJsonSchema": draft_schema
        }
    });


    let raw_text =
        post_gemini_http(http_client, gemini_key, model, body, &usage_totals, "research").await?;

    let json_str = extract_json(&raw_text).unwrap_or(raw_text);
    let value: Value = serde_json::from_str(&json_str).map_err(AppError::Json)?;

    if let Some(searches) = value.get("webSearches").and_then(|v| v.as_array()) {
        println!("[WEB][research] search queries executed:");
        for (idx, entry) in searches.iter().enumerate() {
            if let Some(q) = entry.as_str() {
                println!("  {:>2}. {}", idx + 1, q);
            }
        }
    } else {
        println!("[WEB][research] No explicit webSearches returned.");
    }

    // Deserialize draft as mutable so we can adjust sources.
    let mut draft: DraftWithSources =
        serde_json::from_value(value).map_err(AppError::Json)?;

    let client = http_client.clone();
    let checks = draft.sources.iter().cloned().map(|source| {
        let client = client.clone();
        async move {
            match client
                .get(&source.url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
            {
                Ok(resp) => {
                    let status = resp.status();
                    SourceCheckResult {
                        source,
                        status: Some(status),
                        error: None,
                        ok: status.is_success(),
                    }
                }
                Err(err) => SourceCheckResult {
                    source,
                    status: None,
                    error: Some(truncate(&err.to_string(), 100).to_string()),
                    ok: false,
                },
            }
        }
    });

    let results = join_all(checks).await;

    let mut valid_sources = Vec::new();
    let mut invalid_sources = Vec::new();

    for result in results {
        if result.ok {
            println!(
                "[WEB][verify] {} ({}) status: {}",
                result.source.id,
                result.source.url,
                result.status.unwrap()
            );
            valid_sources.push(result.source);
        } else {
            match (result.status, result.error.as_deref()) {
                (Some(status), _) => println!(
                    "\x1b[31m[WEB][verify] {} ({}) status: {}\x1b[0m",
                    result.source.id,
                    result.source.url,
                    status
                ),
                (None, Some(err_str)) => println!(
                    "\x1b[31m[WEB][verify] {} ({}) status: request failed ({})\x1b[0m",
                    result.source.id,
                    result.source.url,
                    err_str
                ),
                _ => println!(
                    "\x1b[31m[WEB][verify] {} ({}) status: unknown error\x1b[0m",
                    result.source.id,
                    result.source.url
                ),
            }
            invalid_sources.push(result.source.id);
        }
    }

    if !invalid_sources.is_empty() {
        println!(
            "\x1b[31m[WEB][verify][WARN] {} sources removed due to non-200 status: {:?}\x1b[0m",
            invalid_sources.len(),
            invalid_sources
        );
    }

    draft.sources = valid_sources;

    // re-label sequentially: S1, S2, ...
    // for (idx, source) in draft.sources.iter_mut().enumerate() {
    //     source.id = format!("S{}", idx + 1);
    // }

    usage_totals.add_web_searches(draft.sources.len() as u64);

    spinner_done.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(draft)
}