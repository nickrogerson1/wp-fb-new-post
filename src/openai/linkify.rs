use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use std::io::Write;
use crate::{
    errors::AppError,
    models::{Article, DraftWithSources},
    utils::{dbg, dbg_plain, extract_json, truncate, normalize_emdashes},
    models::UsageTotals,
    utils::short_for_spinner,
    openai::http_request::post_openai_http,
};


pub async fn linkify_article_with_chat(
    client: &reqwest::Client,
    openai_key: &str,
    model: &str,
    draft: &DraftWithSources,
    usage_totals: Arc<UsageTotals>,
) -> Result<Article, AppError> {

    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();

    // Full title
    let title_for_msg = draft.title.clone();
    // Truncated title for the spinner line
    let msg = short_for_spinner(&title_for_msg, 60);

    tokio::spawn(async move {
        let mut secs = 0u64;
        loop {
            if done_clone.load(Ordering::Relaxed) {
                // Clear a wide line to be safe
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
            - Never mention the source name, outlet, or “according to …” inside the anchor text. The anchor must describe the claim itself, not who reported it.


            4. Structure & style:
            - Preserve the article’s headings, lists, intro, conclusion, tone, and word count (keep 1000–1500 words).
            - Remove every [Sx] token after link insertion.
            - Do not add tracking params or extra sections.

            5. Output:
            - Return JSON with keys: title, excerpt, contentHtml.
            "#;

        let sources_json = serde_json::to_string(&draft.sources).unwrap_or("[]".into());
        let title_json = serde_json::to_string(&draft.title).unwrap_or("".into());
        let excerpt_json = serde_json::to_string(&draft.excerpt).unwrap_or("".into());
        let content_json = serde_json::to_string(&draft.content_html).unwrap_or("".into());

        let user = format!(
            r#"Here is the draft and its sources.

            SOURCES (JSON):
            {}

            DRAFT (JSON):
            {{"title":{},"excerpt":{},"contentHtml":{}}}

            Return ONLY JSON with keys: title, excerpt, contentHtml."#,
            sources_json, title_json, excerpt_json, content_json
        );

        let payload = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ],
            "response_format": { "type": "json_object" }
        });

        let pretty = serde_json::to_string_pretty(&payload).unwrap_or_default();
        dbg_plain("linkify OpenAI payload", &pretty);

        let v = post_openai_http(
            client,
            openai_key,
            payload,
            usage_totals.clone(),
            "linkify",
        )
        .await?;

        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        if content.is_empty() {
            dbg("linkify empty content", v.to_string());
            return Err(AppError::EmptyOpenAI);
        }

        let json_str = extract_json(&content).unwrap_or(content);
        dbg("linkify content JSON (truncated)", truncate(&json_str, 2000));

        let mut final_article: Article = serde_json::from_str(&json_str).map_err(|e| {
            dbg("linkify content JSON parse error", &e);
            AppError::Json(e)
        })?;

        // Replace em dashes before returning
        final_article.title = normalize_emdashes(&final_article.title);
        final_article.excerpt = normalize_emdashes(&final_article.excerpt);
        final_article.content_html = normalize_emdashes(&final_article.content_html);

        Ok(final_article)
    }
    .await;

    done.store(true, Ordering::Relaxed);

    result
}