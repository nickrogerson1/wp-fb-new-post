use std::sync::Arc;
use crate::{
    errors::AppError,
    models::{Article, SocialAssets, UsageTotals},
    utils::{dbg, extract_json, truncate},
    config::desired_image_url_limit,
    io_utils::upgrade_to_google_advanced,
    openai::http_request::post_openai_http,
};


pub async fn generate_social_assets(
    client: &reqwest::Client,
    openai_key: &str,
    model: &str,
    article: &Article,
    usage_totals: Arc<UsageTotals>,
) -> Result<SocialAssets, AppError> {

    let image_limit = desired_image_url_limit();

    // println!("Image URL limit set to {}", image_limit);

    let system = format!(r#"
        You craft viral social snippets and suggest URLs to suitable images.

        OUTPUT FORMAT
        - Return ONLY JSON with keys:
            * facebookSnippet  (string, <= 500 chars, energetic/clickable, includes a curiosity hook)
            * imageUrls        (array of {} https URLs)

        IMAGE URL RULES
        - Every entry in imageUrls MUST be a Google advanced image search URL.
        - Use this exact base format (no tbm=isch, no other domains):

        https://www.google.com/search?as_st=y&as_q=<QUERY>&as_epq=&as_oq=&as_eq=&imgsz=l&imgar=w&imgcolor=&imgtype=&cr=&as_sitesearch=&as_filetype=&tbs=&udm=2

        where:
            - <QUERY> is your search query, URL-encoded.
            - Spaces may be encoded as + (e.g. neil+young+daryl+hannah+wedding).
            - Do NOT add or remove parameters.
            - Do NOT include tbm=isch or any other extra query parameters.
            - The domain must be www.google.com.

        - All URLs MUST:
            * start with https://www.google.com/search?
            * contain as_st=y, imgsz=l, imgar=w, and udm=2 exactly as shown.
        - Do NOT output any other types of URLs or domains.
        - Do NOT output stock / royalty-free sites; imageUrls must be ONLY these Google search URLs.

        SNIPPET RULES
        - facebookSnippet should:
            * be energetic and clickable, with a strong curiosity hook.
            * contain NO hashtags.
            * use emojis when appropriate to engage the reader.
            * use exactly one sentence per paragraph, with a blank line between paragraphs.
            * end with three down-pointing emojis like: 👇👇👇.
            * never use emdashes (—); use regular hyphens (-) only.
        "#,
        image_limit);

    let content_preview = truncate(&article.content_html, 6000);
    let user = format!(
        r#"Create assets for this KnowYourInstrument article.

        Title: {title}
        Excerpt: {excerpt}

        HTML content (truncated):
        {content}

        Return JSON with keys "facebookSnippet" and "imageUrls"."#,
        title = article.title,
        excerpt = article.excerpt,
        content = content_preview
    );

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "response_format": { "type": "json_object" }
    });

    let v = post_openai_http(
        client,
        openai_key,
        payload,
        usage_totals.clone(),
        "social",
    )
    .await?;

    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();

    if content.is_empty() {
        dbg("generate_social_assets empty content", v.to_string());
        return Err(AppError::EmptyOpenAI);
    }

    let json_str = extract_json(&content).unwrap_or(content);
    // dbg(
    //     "generate_social_assets content JSON (truncated)",
    //     truncate(&json_str, 2000),
    // );

    let mut assets: SocialAssets = serde_json::from_str(&json_str).map_err(AppError::Json)?;

    // Keep only http(s) URLs and drop obvious royalty‑free / stock image domains.
    const BLOCKED_DOMAINS: [&str; 7] = [
        "unsplash.com",
        "pexels.com",
        "pixabay.com",
        "freepik.com",
        "istockphoto.com",
        "shutterstock.com",
        "stock.adobe.com",
    ];


    assets.image_urls.retain(|url| {
        if !url.starts_with("http") {
            return false;
        }
        !BLOCKED_DOMAINS.iter().any(|blocked| url.contains(blocked))
    });

    assets.image_urls.sort();
    assets.image_urls.dedup();

    for u in &mut assets.image_urls {
        if let Some(new_u) = upgrade_to_google_advanced(u) {
            *u = new_u;
        }
    }

    if assets.image_urls.len() > image_limit {
        assets.image_urls.truncate(image_limit);
    }

    Ok(assets)
}


