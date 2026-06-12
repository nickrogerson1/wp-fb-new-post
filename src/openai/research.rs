use futures::future::join_all;
use scraper::{Html, Selector};
use std::sync::{
    Arc,
    atomic::Ordering,
};
use std::time::Duration;
use std::collections::HashSet;
use url::Url;
use crate::{
    errors::AppError,
    models::{
        DraftWithSources, 
        DraftMode, 
        UsageTotals, 
        SourceCheckResult
    },
    utils::{
        collapse_whitespace,
        extract_json,
        short_for_spinner,
        truncate,
        dbg,
        timer_spinner,
        is_banned_domain,
        BANNED_DOMAINS,
    },
    config::SiteConfig,
    openai::http_request::post_openai_http,
};

#[derive(Debug, Clone)]
struct SeedPageExtract {
    url: String,
    title: String,
    text: String,
    ok: bool,
    error: Option<String>,
}

// struct ModeCfg {
//     label: &'static str,
//     min_words: usize,
//     max_words: usize,
//     bullets: &'static str,
//     global_time_rule: Option<&'static str>, // evergreen-only
// }

// fn mode_cfg(mode: DraftMode) -> ModeCfg {
//     match mode {
//         DraftMode::Evergreen => ModeCfg {
//             label: "Evergreen",
//             min_words: 1000,
//             max_words: 1200,
//             bullets: r#"
//                 - Keep the article timeless.
//                 - Do not mention "today", "this week", or the current date.
//                 "#,
//             global_time_rule: Some("Don't mention the current time or date as these are forever green articles."),
//         },
//         DraftMode::News => ModeCfg {
//             label: "News",
//             min_words: 700,
//             max_words: 1000,
//             bullets: r#"
//                 - Write in a news/reporting style: strong lede, what happened, why it matters, key takeaways.
//                 - It is OK to include dates/timelines if sources support them.
//                 - Avoid speculation; separate confirmed facts from analysis.
//                 "#,
//             global_time_rule: None,
//         },
//     }
// }

const MIN_SOURCES_REQUIRED: usize = 10;
const MAX_SEED_PAGES: usize = 10;                 // cap how many seed links we ingest
const MAX_SEED_HTML_BYTES: u64 = 750_000;        // reject huge pages
const MAX_SEED_TEXT_CHARS_PER_PAGE: usize = 2500;
const MAX_SEED_TEXT_CHARS_TOTAL: usize = 8000;

async fn fetch_seed_page_extracts(
    client: &reqwest::Client,
    seed_links: &[String],
) -> Vec<SeedPageExtract> {

    // Dedupe while preserving order
    let mut unique = Vec::<String>::new();
    for u in seed_links {
        if !unique.contains(u) {
            unique.push(u.clone());
        }
        if unique.len() >= MAX_SEED_PAGES {
            break;
        }
    }

    let futs = unique.into_iter().map(|url| {
        let client = client.clone();
        async move {
            // Basic host validation + banned check
            let host = Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()));
            if let Some(ref h) = host {
                if crate::utils::is_banned_domain(h) {
                    return SeedPageExtract {
                        url,
                        title: String::new(),
                        text: String::new(),
                        ok: false,
                        error: Some("banned domain".into()),
                    };
                }
            }

            let resp = match client
                .get(&url)
                .timeout(Duration::from_secs(15))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    return SeedPageExtract {
                        url,
                        title: String::new(),
                        text: String::new(),
                        ok: false,
                        error: Some(format!(
                            "request failed: {}",
                            crate::utils::truncate(&e.to_string(), 140)
                        )),
                    };
                }
            };

            let status = resp.status();
            if !status.is_success() {
                return SeedPageExtract {
                    url,
                    title: String::new(),
                    text: String::new(),
                    ok: false,
                    error: Some(format!("http status {}", status)),
                };
            }

            if let Some(len) = resp.content_length() {
                if len > MAX_SEED_HTML_BYTES {
                    return SeedPageExtract {
                        url,
                        title: String::new(),
                        text: String::new(),
                        ok: false,
                        error: Some(format!("page too large ({} bytes)", len)),
                    };
                }
            }

            let html = match resp.text().await {
                Ok(t) => t,
                Err(e) => {
                    return SeedPageExtract {
                        url,
                        title: String::new(),
                        text: String::new(),
                        ok: false,
                        error: Some(format!(
                            "read body failed: {}",
                            crate::utils::truncate(&e.to_string(), 140)
                        )),
                    };
                }
            };

            // Extract <title> best-effort
            let title = {
                let doc = Html::parse_document(&html);
                let sel = Selector::parse("title").ok();
                sel.and_then(|s| doc.select(&s).next().map(|n| n.text().collect::<String>()))
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            };

            // HTML -> readable text
            let mut text = html2text::from_read(html.as_bytes(), 120).unwrap_or_default();
            text = crate::utils::collapse_whitespace(&text);

            // Cap per-page text safely (byte cap)
            if text.len() > MAX_SEED_TEXT_CHARS_PER_PAGE {
                truncate_to_char_boundary_in_place(&mut text, MAX_SEED_TEXT_CHARS_PER_PAGE);
            }

            SeedPageExtract {
                url,
                title,
                text,
                ok: true,
                error: None,
            }
        }
    });

    join_all(futs).await
}

fn truncate_to_char_boundary_in_place(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut n = max_bytes;
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    s.truncate(n);
}

fn build_seed_extracts_block(extracts: Vec<SeedPageExtract>) -> String {
    let mut out = String::new();
    let mut total_bytes = 0usize;

    for ex in extracts {
        if !ex.ok {
            eprintln!(
                "\x1b[31m[SEED][WARN] {} fetch failed: {}\x1b[0m",
                ex.url,
                ex.error.unwrap_or_else(|| "unknown error".into())
            );
            continue;
        }

        let mut chunk = format!(
            "URL: {}\nTITLE: {}\nTEXT: {}\n---\n",
            ex.url,
            ex.title,
            ex.text
        );

        // Cap total output (in bytes) but truncate safely on UTF-8 boundaries
        if total_bytes + chunk.len() > MAX_SEED_TEXT_CHARS_TOTAL {
            let remaining = MAX_SEED_TEXT_CHARS_TOTAL.saturating_sub(total_bytes);
            if remaining == 0 {
                break;
            }
            truncate_to_char_boundary_in_place(&mut chunk, remaining);
        }

        total_bytes += chunk.len();
        out.push_str(&chunk);

        if total_bytes >= MAX_SEED_TEXT_CHARS_TOTAL {
            break;
        }
    }

    if out.trim().is_empty() {
        "None.\n".to_string()
    } else {
        out
    }
}


fn build_research_prompts(
    site_cfg: &SiteConfig,
    topic: &str,
    seed_links: &[String],
    seed_extracts_block: &str,
    mode: DraftMode,
) -> (String, String, String) {
    let site_context = &site_cfg.site_context;

    let banned_domains_section = if BANNED_DOMAINS.is_empty() {
        String::from("Banned domains: (none)\n")
    } else {
        let list = BANNED_DOMAINS
            .iter()
            .map(|d| format!("- {}", d))
            .collect::<Vec<_>>()
            .join("\n");
        format!("Banned domains:\n{}\n", list)
    };

    let seed_links_section = if seed_links.is_empty() {
        "SeedLinks: []\n".to_string()
    } else {
        let joined = seed_links
            .iter()
            .map(|u| format!("- {}", u))
            .collect::<Vec<_>>()
            .join("\n");
        format!("SeedLinks (editor-provided):\n{}\n", joined)
    };

    // let system = format!(
    //     r#"Role:
    //     You are a senior content writer and music expert.
    //     Write comprehensive, accurate, and helpful articles.
    //     Don't mention that the article is "sourced" or "well-sourced".
    //     Don't repeat sections or phrases. Try to keep each section/paragraph well differentiated.

    //     Output:
    //     Return ONLY a JSON object with keys: title, excerpt, contentHtml, sources, tags, slug.

    //     contentHtml must be valid HTML for WordPress, with no <html>, <head>, or <body>.

    //     {mode_instructions}

    //     title <= 100 chars; excerpt <= 160 chars.
    //     Use H2/H3; short intro and concise conclusion.
    //     Fun but professional tone tailored to the site.
    //     Modify the title to make it more eye-catching / engaging.
    //     Prefer short paragraphs and scannable lists or HTML tables where useful.
    //     HTML Table headers should be centered.
    //     Use HTML quotes and blockquotes where appropriate to add effect/emphasis.
    //     Always attribute quotes and blockquotes to their original source/speaker.
    //     Paragraphs should be 2-3 sentences in length.
    //     Be factual. If uncertain, generalize rather than invent specifics.
    //     Do NOT include <a> tags yet.
    //     Don't mention the current time or date as these are forever green articles.
    //     Never use emdashes (—); use regular hyphens (-) only.

    //     Citations:
    //     -Use [S1], [S2]... in contentHtml.
    //     -Return 'sources' array of {{id, url, title}}.
    //     -Deduplicate; No tracking params.
    //     -Use each domain at most once. If you already cited a site, pick another source from a different domain.

    //     Seed links:
    //     - You are given SeedLinks plus Seed Link Extracts (page text fetched by the system).
    //     - Use the Seed Link Extracts as input for facts/claims where relevant.
    //     - If you use info from a seed extract, you MUST include that URL in sources and cite it in-text.
    //     - Still call httpHead for any URL you plan to cite (including seed links) to ensure it works and to normalize finalUrl.

    //     When choosing citations:
    //     - If there are high-quality, relevant pages available on knowyourinstrument.com, prefer to use them as sources.
    //     - Only use wikipedia links a maximum of once and only when they are directly relevant to the topic.
    //     - Place such pages early in the sources list (e.g., [S1], [S2]) when they directly support a claim.
    //     - ALWAYS try to link to a source when there was an interview, quote, claim, report or opinion to back it up.
    //     - Do NOT invent URLs on knowyourinstrument.com; only use URLs that actually come from search results.
    //     - Do not use sources multiple times in the same article. Find the most relevant place for each source and use it once.
    //     - Use each source in exactly one sentence. Never attach more than one citation to the same sentence.
    //     - Don't quote the source as part of the anchor text.

    //     Source verification rules:
    //     - Call httpHead for every source you plan to include.
    //     - Use the finalUrl from the response if it differs from what you supplied, unless ok: false.
    //     - If the tool reports ok: false, discard that source and pick another.
    //     - Keep going until you have at least {MIN_SOURCES_REQUIRED} distinct, valid sources.
    //     {banned_domains_section}

    //     {seed_links_section}
    //     Seed Link Extracts (machine-extracted page text; may be truncated):
    //     {seed_extracts_block}

    //     Site context: {site_context}
    // "#
    // );

    let mode_instructions = match mode {
        DraftMode::Evergreen => r#"
            Mode: Evergreen
            - Keep the article timeless.
            - Do not mention "today", "this week", "recently", or the current date/time.
            - Do not use relative time language (e.g., "currently", "right now") unless it is part of a sourced quote.
            Length:
            - contentHtml should be at least 1000 words.
            - contentHtml must not exceed 1200 words.
            "#,
        DraftMode::News => r#"
            Mode: News
            - Write in a news/reporting style: strong lede, what happened, why it matters, key takeaways.
            - It is OK to include dates/timelines ONLY if supported by the provided sources/seed extracts.
            - Avoid speculation; separate confirmed facts from analysis. If something is unconfirmed, say so clearly.
            Length:
            - contentHtml should be at least 700 words.
            - contentHtml must not exceed 1000 words.
            "#,
    };

    let system = format!(
        r#"Role:
        You are a senior content writer and music expert.
        Write comprehensive, accurate, and helpful articles.
        Don't mention that the article is "sourced" or "well-sourced".
        Don't repeat sections or phrases. Keep each section/paragraph well differentiated.

        Output:
        Return ONLY a JSON object with keys: title, excerpt, contentHtml, sources, tags, slug.

        HTML requirements:
        - contentHtml must be valid HTML for WordPress, with no <html>, <head>, or <body>.
        - Use H2/H3 headings.
        - Short intro and concise conclusion.
        - Prefer short paragraphs (2-3 sentences).
        - Prefer scannable lists or HTML tables where useful.
        - HTML table headers should be centered.
        - Use quotes and blockquotes where appropriate to add effect/emphasis.
        - Always attribute quotes and blockquotes to their original source/speaker.
        - Do NOT include <a> tags yet.
        - Never use emdashes (—); use regular hyphens (-) only.

        {mode_instructions}

        Title/excerpt:
        - title <= 100 chars.
        - excerpt <= 160 chars.
        - Modify the title to be more eye-catching / engaging, but keep it accurate.

        Accuracy:
        - Be factual. If uncertain, generalize rather than invent specifics.
        - Do not invent names, dates, stats, or claims that are not supported by sources/seed extracts.

        Citations:
        - Use [S1], [S2]... markers in contentHtml.
        - Return 'sources' as an array of {{id, url, title}}.
        - Deduplicate sources; remove tracking params from URLs.
        - Use each domain at most once across the sources list.
        - Use each source in exactly one sentence in the article.
        - Never attach more than one citation to the same sentence.

        Seed links:
        - You may be given SeedLinks plus Seed Link Extracts (page text fetched by the system).
        - Use the Seed Link Extracts as input for facts/claims where relevant.
        - If you use info from a seed extract, you MUST include that URL in sources and cite it in-text.
        - You must still call httpHead for any URL you plan to cite (including seed links) to ensure it works and to normalize finalUrl.

        When choosing citations:
        - If there are high-quality, relevant pages available on knowyourinstrument.com, prefer them as sources.
        - Do NOT invent URLs on knowyourinstrument.com; only use URLs that actually exist.
        - Only use Wikipedia at most once and only when directly relevant.
        - Prefer primary/authoritative sources when possible.
        - Always try to cite a source when there was an interview, quote, claim, report, or opinion to back it up.
        - If you need more sources, keep searching/adding sources until requirements are met.

        Source verification rules:
        - Call httpHead for every source you plan to include.
        - Use finalUrl from httpHead if it differs from what you supplied, unless ok: false.
        - If ok: false (including cross-domain redirect or homepage redirect), discard that source and pick another.
        - Keep going until you have at least {MIN_SOURCES_REQUIRED} distinct, valid sources.
        {banned_domains_section}

        {seed_links_section}
        Seed Link Extracts (machine-extracted page text; may be truncated):
        {seed_extracts_block}

        Site context: {site_context}
        "#,
    );

    let user1 = format!(r#"Topic: "{topic}""#);

    let user2 = format!(
        r#"Create the most interesting, engaging, and informative article possible on this topic.
        Feel free to include edgy content or provocative claims to capture reader interest.
        Use the Seed Link Extracts as part of your research.
        Return JSON with keys: title, excerpt, contentHtml, sources, tags, slug.
        - tags: array 4-6 most relevant lower-case WordPress tags. Use spaces between the words not dashes (-).
        - slug: URL-friendly suggestion (dash-separated, <= 8 words) which is related to the title.
        - You must return at least {MIN_SOURCES_REQUIRED} verified sources. Whenever you pick a source,
          call httpHead to confirm it works; if it fails, replace it.
        Do not include code fences."#
    )
    .to_string();

    (system, user1, user2)
}


// fn build_research_prompts(site_cfg: &SiteConfig, topic: &str) -> (String, String, String) {
//     let site_context = &site_cfg.site_context;

//     let banned_domains_section = if BANNED_DOMAINS.is_empty() {
//         String::from("Banned domains: (none)\n")
//     } else {
//         let list = BANNED_DOMAINS
//             .iter()
//             .map(|d| format!("- {}", d))
//             .collect::<Vec<_>>()
//             .join("\n");
//         format!("Banned domains:\n{}\n", list)
//     };

//     let system = format!(
//         r#"Role:
//         You are a senior content writer and music expert.
//         Write comprehensive, accurate, and helpful articles.
//         Don't mention that the article is "sourced" or "well-sourced".
//         Don't repeat sections or phrases. Try to keep each section/paragraph well differentiated.

//         Output:
//         Return ONLY a JSON object with keys: title, excerpt, contentHtml, sources, tags, slug.

//         contentHtml must be valid HTML for WordPress, with no <html>, <head>, or <body>.
//         contentHtml should be at least 1000 words.
//         contentHtml must not exceed 1200 words.
//         title <= 100 chars; excerpt <= 160 chars.
//         Use H2/H3; short intro and concise conclusion.
//         Fun but professional tone tailored to the site.
//         Modify the title to make it more eye-catching / engaging.
//         Prefer short paragraphs and scannable lists or HTML tables where useful.
//         HTML Table headers should be centered.
//         Use HTML quotes and blockquotes where appropriate to add effect/emphasis.
//         Always attribute quotes and blockquotes to their original source/speaker.
//         Paragraphs should be 2-3 sentences in length.
//         Be factual. If uncertain, generalize rather than invent specifics.
//         Do NOT include <a> tags yet.
//         Don't mention the current time or date as these are forever green articles.
//         Never use emdashes (—); use regular hyphens (-) only.

//         Citations:
//         -Use [S1], [S2]... in contentHtml.
//         -Return 'sources' array of {{id, url, title}}.
//         -Deduplicate; No tracking params.
//         -Use each domain at most once. If you already cited a site, pick another source from a different domain.


//         When using web_search (or googleSearch) and choosing citations:

//         - If there are high-quality, relevant pages available on knowyourinstrument.com, prefer to use them as sources.
//         - Only use wikipedia links a maximum of once and only when they are directly relevant to the topic.
//         - Place such pages early in the sources list (e.g., [S1], [S2]) when they directly support a claim.
//         - ALWAYS try to link to a source when there was an interview, quote, claim, report or opinion to back it up.
//         - Do NOT invent URLs on knowyourinstrument.com; only use URLs that actually come from search results.
//         - If no suitable page exists on knowyourinstrument.com, fall back to other primary/authoritative sources.
//         - Do not use sources multiple times in the same article. Find the most relevant place for each source and use it once.
//         - Use each source in exactly one sentence. Never attach more than one citation to the same sentence.
//         - If you need more sources, then keep using search to find them.
//         - Don't quote the source as part of the anchor text.

//         - Call httpHead for every source. Use the finalUrl from the response if it differs from what you supplied, 
//           unless the tool marks ok: false (e.g., cross-domain redirect).
//           If the tool reports anything else, discard that source and keep searching.
//         - Keep searching until you have at least {MIN_SOURCES_REQUIRED} distinct, valid sources.
//         {banned_domains_section}

//         Site context: {site_context}
//         "#
//     );

//     let user1 = format!(r#"Topic: "{topic}""#);

//     let user2 = format!(r#"Create the most interesting, engaging, and informative article possible on this topic.
//         Feel free to include edgy content or provocative claims to capture reader interest.
//         Research the latest credible information using web search.
//         Prioritize primary/authoritative sources.
//         Return JSON with keys: title, excerpt, contentHtml, sources, tags, slug.
//         - tags: array 4-6 most relevant lower-case WordPress tags. Use spaces between the words not dashes (-).
//         - slug: URL-friendly suggestion (dash-separated, <= 8 words) which is related to the title.
//         - You must return at least {MIN_SOURCES_REQUIRED} verified sources. Whenever you pick a source, 
//           call httpHead to confirm it works; if it fails, keep searching.
//         Do not include code fences."#)
//         .to_string();

//     (system, user1, user2)
// }


// fn build_research_prompts(
//     site_cfg: &SiteConfig,
//     topic: &str,
//     seed_links: &[String],
//     mode: crate::models::DraftMode,
// ) -> (String, String, String) {
//     let site_context = &site_cfg.site_context;

//     let banned_domains_section = if BANNED_DOMAINS.is_empty() {
//         String::from("Banned domains: (none)\n")
//     } else {
//         let list = BANNED_DOMAINS
//             .iter()
//             .map(|d| format!("- {}", d))
//             .collect::<Vec<_>>()
//             .join("\n");
//         format!("Banned domains:\n{}\n", list)
//     };

//     let seed_links_section = if seed_links.is_empty() {
//         "SeedLinks: []\n".to_string()
//     } else {
//         let joined = seed_links
//             .iter()
//             .map(|u| format!("- {}", u))
//             .collect::<Vec<_>>()
//             .join("\n");
//         format!(
//             "SeedLinks (editor-provided; validate each with httpHead before using):\n{}\n",
//             joined
//         )
//     };

//     let mode_instructions = match mode {
//         DraftMode::Evergreen => r#"
//             Mode: Evergreen
//             - Do not mention "today", "this week", or the current date.
//             - Keep the article timeless.
//             - contentHtml should be at least 1000 words.
//             - contentHtml must not exceed 1200 words.
//         "#,
//         DraftMode::News => r#"
//             Mode: News
//             - Write in a news/reporting style: strong lede, what happened, why it matters, key takeaways.
//             - It is OK to include dates/timelines if sources support them.
//             - Avoid speculation; separate confirmed facts from analysis.
//             - contentHtml should be at least 700 words.
//             - contentHtml must not exceed 800 words.
//         "#,
//             };

//     let system = format!(
//         r#"Role:
//         You are a senior content writer and music expert.
//         Write comprehensive, accurate, and helpful articles.
//         Don't mention that the article is "sourced" or "well-sourced".
//         Don't repeat sections or phrases. Try to keep each section/paragraph well differentiated.

//         Output:
//         Return ONLY a JSON object with keys: title, excerpt, contentHtml, sources, tags, slug.

//         contentHtml must be valid HTML for WordPress, with no <html>, <head>, or <body>.
//         title <= 100 chars; excerpt <= 160 chars.
//         Use H2/H3; short intro and concise conclusion.
//         Fun but professional tone tailored to the site.
//         Modify the title to make it more eye-catching / engaging.
//         Prefer short paragraphs and scannable lists or HTML tables where useful.
//         HTML Table headers should be centered.
//         Use HTML quotes and blockquotes where appropriate to add effect/emphasis.
//         Always attribute quotes and blockquotes to their original source/speaker.
//         Paragraphs should be 2-3 sentences in length.
//         Be factual. If uncertain, generalize rather than invent specifics.
//         Do NOT include <a> tags yet.
//         Never use emdashes (—); use regular hyphens (-) only.

//         {mode_instructions}

//         Citations:
//         - Use [S1], [S2]... in contentHtml.
//         - Return 'sources' array of {{id, url, title}}.
//         - Deduplicate; No tracking params.
//         - Use each domain at most once. If you already cited a site, pick another source from a different domain.

//         When using web_search (or googleSearch) and choosing citations:
//         - If there are high-quality, relevant pages available on knowyourinstrument.com, prefer to use them as sources.
//         - Only use wikipedia links a maximum of once and only when they are directly relevant to the topic.
//         - Place such pages early in the sources list (e.g., [S1], [S2]) when they directly support a claim.
//         - ALWAYS try to link to a source when there was an interview, quote, claim, report or opinion to back it up.
//         - Do NOT invent URLs on knowyourinstrument.com; only use URLs that actually come from search results.
//         - If no suitable page exists on knowyourinstrument.com, fall back to other primary/authoritative sources.
//         - Do not use sources multiple times in the same article. Find the most relevant place for each source and use it once.
//         - Use each source in exactly one sentence. Never attach more than one citation to the same sentence.
//         - If you need more sources, then keep using search to find them.
//         - Don't quote the source as part of the anchor text.

//         Seed links rules:
//         - You may be given SeedLinks chosen by an editor.
//         - Call httpHead for EVERY SeedLinks URL before writing.
//         - If httpHead returns ok: true, you may use it as a source if relevant (counts toward required sources).
//         - If ok: false, discard it and do not cite it.

//         Source verification rules:
//         - Call httpHead for every source you plan to include.
//         - Use the finalUrl from the response if it differs from what you supplied, unless ok: false.
//         - If the tool reports ok: false (including cross-domain redirects / homepage redirects), discard that source.
//         - Keep searching until you have at least {MIN_SOURCES_REQUIRED} distinct, valid sources.
//         {banned_domains_section}
//         {seed_links_section}

//         Site context: {site_context}
//         "#
//             );

//         let user1 = format!(r#"Topic: "{topic}""#);

//         let user2 = format!(
//                 r#"Create the most interesting, engaging, and informative article possible on this topic.
//         Research the latest credible information using web search.
//         Prioritize primary/authoritative sources.
//         Return JSON with keys: title, excerpt, contentHtml, sources, tags, slug.
//         - tags: array 4-6 most relevant lower-case WordPress tags. Use spaces between the words not dashes (-).
//         - slug: URL-friendly suggestion (dash-separated, <= 8 words) which is related to the title.
//         - You must return at least {MIN_SOURCES_REQUIRED} verified sources. Whenever you pick a source,
//         call httpHead to confirm it works; if it fails, keep searching.
//         Do not include code fences."#
//     );

//     (system, user1, user2)
// }



pub async fn research_draft(
    client: &reqwest::Client,
    openai_key: &str,
    site_cfg: &SiteConfig,
    model: &str,
    topic: &str,
    seed_links: &[String],
    mode: DraftMode,
    usage_totals: Arc<UsageTotals>,
) -> Result<DraftWithSources, AppError> {
    // Spinner setup via shared utility
    let topic_for_msg = collapse_whitespace(topic);
    let spinner_label = short_for_spinner(&topic_for_msg, 60);
    let spinner_done = timer_spinner("Researching", spinner_label);

    let result: Result<DraftWithSources, AppError> = async {
        // Fetch and build seed extracts block (best-effort)
        let seed_extracts = fetch_seed_page_extracts(client, seed_links).await;
        let seed_extracts_block = build_seed_extracts_block(seed_extracts);

        let (system, user1, user2) =
            build_research_prompts(site_cfg, topic, seed_links, &seed_extracts_block, mode);

        let payload = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user1 },
                { "role": "user", "content": user2 }
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "draft_schema",
                    "schema": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "excerpt": { "type": "string" },
                        "contentHtml": { "type": "string" },
                        "sources": {
                        "type": "array",
                        "minItems": MIN_SOURCES_REQUIRED,
                        "items": {
                            "type": "object",
                            "properties": {
                            "id": { "type": "string" },
                            "url": { "type": "string", "format": "uri" },
                            "title": { "type": "string" }
                            },
                            "required": ["id","url","title"]
                        }
                        }
                    },
                    "required": ["title","excerpt","contentHtml","sources"]
                    }
                }
            },
            "tool_choice": "auto",
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "httpHead",
                        "description": "Fetch a URL (HEAD or GET), follow same-domain redirects up to 5 hops, and report the final URL, HTTP status, and whether it's OK.",
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
                }
            ]
        });

        let v = post_openai_http(
            client,
            openai_key,
            payload,
            usage_totals.clone(),
            "research",
        )
        .await?;

        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();

        if content.is_empty() {
            dbg("research_draft empty content", v.to_string());
            return Err(AppError::EmptyOpenAI);
        }

        let json_str = extract_json(&content).unwrap_or(content);
        let mut draft: DraftWithSources = serde_json::from_str(&json_str).map_err(|e| {
            dbg("research_draft content JSON parse error", &e);
            AppError::Json(e)
        })?;

        // --- your existing verification logic (unchanged) ---
        let client_clone = client.clone();
        let checks = draft.sources.iter().cloned().map(|source| {
            let client = client_clone.clone();
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
        let mut invalid_ids = Vec::new();
        let mut seen_domains = HashSet::<String>::new();

        for result in results {
            if result.ok {
                let Some(host) = Url::parse(&result.source.url)
                    .ok()
                    .and_then(|u| u.host_str().map(|s| s.to_ascii_lowercase()))
                else {
                    eprintln!(
                        "\x1b[31m[WEB][verify] {} ({}) rejected: invalid host\x1b[0m",
                        result.source.id, result.source.url
                    );
                    invalid_ids.push(result.source.id);
                    continue;
                };

                if is_banned_domain(&host) {
                    eprintln!(
                        "\x1b[31m[WEB][verify] {} ({}) rejected: banned domain\x1b[0m",
                        result.source.id, result.source.url
                    );
                    invalid_ids.push(result.source.id);
                    continue;
                }

                if seen_domains.contains(&host) {
                    eprintln!(
                        "\x1b[33m[WEB][verify] {} ({}) rejected: duplicate domain {}\x1b[0m",
                        result.source.id, result.source.url, host
                    );
                    invalid_ids.push(result.source.id);
                    continue;
                }

                seen_domains.insert(host);

                println!(
                    "\x1b[32m[WEB][verify] {} ({}) status: {}\x1b[0m",
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
                invalid_ids.push(result.source.id);
            }
        }

        if !invalid_ids.is_empty() {
            println!(
                "\x1b[31m[WEB][verify][WARN] {} sources removed due to non-200 status or being a dupe: {:?}\x1b[0m",
                invalid_ids.len(),
                invalid_ids
            );
        }

        draft.sources = valid_sources;

        // Re-label surviving sources sequentially
        for (idx, source) in draft.sources.iter_mut().enumerate() {
            source.id = format!("S{}", idx + 1);
        }

        let num_sources = draft.sources.len() as u64;
        println!("[WEB][research] sources for this topic: {}", num_sources);
        usage_totals.add_web_searches(num_sources);

        Ok(draft)
    }
    .await;

    spinner_done.store(true, Ordering::Relaxed);
    result
}

// pub async fn research_draft(
//     client: &reqwest::Client,
//     openai_key: &str,
//     site_cfg: &SiteConfig,
//     model: &str,
//     topic: &str,
//     usage_totals: Arc<UsageTotals>,
// ) -> Result<DraftWithSources, AppError> {
//     // Spinner setup via shared utility
//     let topic_for_msg = collapse_whitespace(topic);
//     let spinner_label = short_for_spinner(&topic_for_msg, 60);
//     let spinner_done = timer_spinner("Researching", spinner_label);

//     let result: Result<DraftWithSources, AppError> = async {
//         let (system, user1, user2) = build_research_prompts(site_cfg, topic);

//         // println!("System prompt for research_draft:\n{}", &system);

//         let payload = serde_json::json!({
//             "model": model,
//             "messages": [
//                 { "role": "system", "content": system },
//                 { "role": "user", "content": user1 },
//                 { "role": "user", "content": user2 }
//             ],
//             // "response_format": { "type": "json_object" },
//             "response_format": {
//                 "type": "json_schema",
//                 "json_schema": {
//                     "name": "draft_schema",
//                     "schema": {
//                     "type": "object",
//                     "properties": {
//                         "title": { "type": "string" },
//                         "excerpt": { "type": "string" },
//                         "contentHtml": { "type": "string" },
//                         "sources": {
//                         "type": "array",
//                         "minItems": MIN_SOURCES_REQUIRED,
//                         "items": {
//                             "type": "object",
//                             "properties": {
//                             "id": { "type": "string" },
//                             "url": { "type": "string", "format": "uri" },
//                             "title": { "type": "string" }
//                             },
//                             "required": ["id","url","title"]
//                         }
//                         }
//                     },
//                     "required": ["title","excerpt","contentHtml","sources"]
//                     }
//                 }
//             },
//             "tool_choice": "auto",
//             "tools": [
//                 {
//                     "type": "function",
//                     "function": {
//                         "name": "httpHead",
//                         "description": "Fetch a URL (HEAD or GET), follow same-domain redirects up to 5 hops, and report the final URL, HTTP status, and whether it's OK.",
//                         "parameters": {
//                             "type": "object",
//                             "properties": {
//                                 "url": {
//                                     "type": "string",
//                                     "description": "Fully-qualified URL to verify",
//                                     "format": "uri"
//                                 },
//                                 "method": {
//                                     "type": "string",
//                                     "enum": ["HEAD", "GET"],
//                                     "description": "HTTP method to use (default HEAD)"
//                                 }
//                             },
//                             "required": ["url"]
//                         }
//                     }
//                 }
//             ]
//         });

//         let v = post_openai_http(
//             client,
//             openai_key,
//             payload,
//             usage_totals.clone(),
//             "research",
//         )
//         .await?;

//         let content = v["choices"][0]["message"]["content"]
//             .as_str()
//             .unwrap_or("")
//             .trim()
//             .to_string();

//         if content.is_empty() {
//             dbg("research_draft empty content", v.to_string());
//             return Err(AppError::EmptyOpenAI);
//         }

//         let json_str = extract_json(&content).unwrap_or(content);
//         let mut draft: DraftWithSources = serde_json::from_str(&json_str).map_err(|e| {
//             dbg("research_draft content JSON parse error", &e);
//             AppError::Json(e)
//         })?;

//         let client_clone = client.clone();
//         let checks = draft.sources.iter().cloned().map(|source| {
//             let client = client_clone.clone();
//             async move {
//                 match client
//                     .get(&source.url)
//                     .timeout(Duration::from_secs(10))
//                     .send()
//                     .await
//                 {
//                     Ok(resp) => {
//                         let status = resp.status();
//                         SourceCheckResult {
//                             source,
//                             status: Some(status),
//                             error: None,
//                             ok: status.is_success(),
//                         }
//                     }
//                     Err(err) => SourceCheckResult {
//                         source,
//                         status: None,
//                         error: Some(truncate(&err.to_string(), 100).to_string()),
//                         ok: false,
//                     },
//                 }
//             }
//         });

//         let results = join_all(checks).await;
//         let mut valid_sources = Vec::new();
//         let mut invalid_ids = Vec::new();
//         let mut seen_domains = HashSet::<String>::new();

//         for result in results {
//             if result.ok {
//                 let Some(host) = Url::parse(&result.source.url)
//                     .ok()
//                     .and_then(|u| u.host_str().map(|s| s.to_ascii_lowercase()))
//                 else {
//                     eprintln!(
//                         "\x1b[31m[WEB][verify] {} ({}) rejected: invalid host\x1b[0m",
//                         result.source.id, result.source.url
//                     );
//                     invalid_ids.push(result.source.id);
//                     continue;
//                 };

//                 if is_banned_domain(&host) {
//                     eprintln!(
//                         "\x1b[31m[WEB][verify] {} ({}) rejected: banned domain\x1b[0m",
//                         result.source.id, result.source.url
//                     );
//                     invalid_ids.push(result.source.id);
//                     continue;
//                 }

//                 if seen_domains.contains(&host) {
//                     eprintln!(
//                         "\x1b[33m[WEB][verify] {} ({}) rejected: duplicate domain {}\x1b[0m",
//                         result.source.id, result.source.url, host
//                     );
//                     invalid_ids.push(result.source.id);
//                     continue;
//                 }

//                 seen_domains.insert(host);

//                 println!(
//                     "\x1b[32m[WEB][verify] {} ({}) status: {}\x1b[0m",
//                     result.source.id,
//                     result.source.url,
//                     result.status.unwrap()
//                 );
//                 valid_sources.push(result.source);
//             } else {
//                 match (result.status, result.error.as_deref()) {
//                     (Some(status), _) => println!(
//                         "\x1b[31m[WEB][verify] {} ({}) status: {}\x1b[0m",
//                         result.source.id,
//                         result.source.url,
//                         status
//                     ),
//                     (None, Some(err_str)) => println!(
//                         "\x1b[31m[WEB][verify] {} ({}) status: request failed ({})\x1b[0m",
//                         result.source.id,
//                         result.source.url,
//                         err_str
//                     ),
//                     _ => println!(
//                         "\x1b[31m[WEB][verify] {} ({}) status: unknown error\x1b[0m",
//                         result.source.id,
//                         result.source.url
//                     ),
//                 }
//                 invalid_ids.push(result.source.id);
//             }
//         }

//         if !invalid_ids.is_empty() {
//             println!(
//                 "\x1b[31m[WEB][verify][WARN] {} sources removed due to non-200 status or being a dupe: {:?}\x1b[0m",
//                 invalid_ids.len(),
//                 invalid_ids
//             );
//         }

//         draft.sources = valid_sources;

//         // Re-label surviving sources sequentially
//         for (idx, source) in draft.sources.iter_mut().enumerate() {
//             source.id = format!("S{}", idx + 1);
//         }

//         let num_sources = draft.sources.len() as u64;
//         println!("[WEB][research] sources for this topic: {}", num_sources);
//         usage_totals.add_web_searches(num_sources);

//         // draft.content_html = enforce_citation_rules(&draft.content_html);

//         Ok(draft)
//     }
//     .await;

//     spinner_done.store(true, Ordering::Relaxed);
//     result
// }
