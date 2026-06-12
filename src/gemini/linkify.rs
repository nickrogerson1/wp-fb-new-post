use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use std::io::Write;
use crate::{
    errors::AppError,
    models::{Article, DraftWithSources},
    utils::{dbg, dbg_plain, extract_json, truncate},
    models::UsageTotals,
    utils::short_for_spinner,
    gemini::http_request::post_gemini_http,
};


pub async fn linkify_article_with_chat(
    client: &reqwest::Client,
    gemini_key: &str,
    model: &str,
    draft: &DraftWithSources,
    usage_totals: Arc<UsageTotals>,
) -> Result<Article, AppError> {
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();

    // Spinner
    let title_for_msg = draft.title.clone();
    let msg = short_for_spinner(&title_for_msg, 60);

    tokio::spawn(async move {
        let mut secs = 0u64;
        loop {
            if done_clone.load(Ordering::Relaxed) {
                eprint!("\r{:width$}\r", "", width = 200);
                let _ = std::io::stderr().flush();
                break;
            }

            eprint!("\rLinkifying \"{}\"... {}s", msg, secs);
            let _ = std::io::stderr().flush();
            secs += 1;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let result: Result<Article, AppError> = async {
        let system = r#"
            You transform a draft article into final WordPress-ready HTML by embedding sources as <a> tags.

            Goal:
            Turn each bracketed citation [Sx] into a natural inline link on existing words inside the same sentence or bullet — never a dangling link at the end.

            Rules:
            1. Mapping:
            - You receive SOURCES: [{ "id":"S1","url":"…","title":"…" }, …].
            - Each [Sx] token references one source. Match ids exactly.
            - If the the [Sx] is missing, then ignore it as the link was invalid.

            2. Anchor placement (critical):
            - Locate the most relevant existing word/phrase in the sentence that the citation supports and wrap that text in <a>.
            - If the citation marker sits at the end of the sentence, rewrite the sentence (lightly) so the anchor sits earlier; do not leave the link trailing punctuation.
            - Never wrap only punctuation or the space before the [Sx].
            - Never invent a new trailing phrase just to hold the link.
            - Example:
                    Draft: “Clapton’s slow-burn tone became his signature. [S3]”
                    ✅ Output: “Clapton’s <a href='…'>slow-burn tone</a> became his signature.”
                    ❌ Output: “Clapton’s slow-burn tone became his signature <a href='…'></a>.”

            3. Anchor text quality:
            - Use concise, descriptive text reflecting the claim (“slow-burn tone,” “touring budgets,” etc.).
            - Do not use the raw URL, “source,” “reference,” or the domain name.
            - Each source should appear only once.

            4. Structure & style:
            - Preserve the article’s headings, lists, intro, conclusion, tone, and word count (keep 1000–1500 words).
            - Remove every [Sx] token after link insertion.
            - Do not add tracking params or extra sections.

            5. Output:
            - Return JSON with keys: title, excerpt, contentHtml, tags, slug.
            "#;

        let sources_json = serde_json::to_string(&draft.sources).unwrap_or_else(|_| "[]".into());
        let title_json   = serde_json::to_string(&draft.title).unwrap_or_else(|_| "null".into());
        let excerpt_json = serde_json::to_string(&draft.excerpt).unwrap_or_else(|_| "null".into());
        let content_json = serde_json::to_string(&draft.content_html).unwrap_or_else(|_| "null".into());

        let user = format!(
            r#"Here is the draft and its sources.

            SOURCES (JSON):
            {}

            DRAFT (JSON):
            {{"title":{},"excerpt":{},"contentHtml":{}}}

            Return ONLY JSON with keys: title, excerpt, contentHtml, tags, slug."#,
            sources_json, title_json, excerpt_json, content_json
        );

        // ---------- Gemini request ----------
        let article_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "excerpt": { "type": "string" },
                "contentHtml": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } },
                "slug": { "type": "string" }
            },
            "required": ["title", "excerpt", "contentHtml", "tags", "slug"]
        });

        let body = serde_json::json!({
            "system_instruction": { "parts": [{ "text": system }] },
            "contents": [{ "role": "user", "parts": [{ "text": user }] }],
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseJsonSchema": article_schema
            }
        });


        let pretty = serde_json::to_string_pretty(&body).unwrap_or_default();
        dbg_plain("linkify gemini payload", &pretty);

        let raw_text = post_gemini_http(client, gemini_key, &model, body, &usage_totals, "research").await?;

        let json_str = extract_json(&raw_text).unwrap_or(raw_text);
        dbg("linkify content JSON (truncated)", truncate(&json_str, 1000));

        let final_article: Article = serde_json::from_str(&json_str).map_err(|e| {
            dbg("linkify content JSON parse error", &e);
            AppError::Json(e)
        })?;

        Ok(final_article)
    }
    .await;

    done.store(true, Ordering::Relaxed);
    result
}