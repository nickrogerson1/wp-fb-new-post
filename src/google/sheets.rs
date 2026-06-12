use reqwest::Client;
use url::form_urlencoded;
use crate::{
    google::auth::google_token_provider,
    errors::AppError,
    utils::{collapse_whitespace, truncate},
    models::SheetTopic,
    models::SocialAssetRow,
    models::GoogleValuesResponse
};

pub async fn load_topics_from_sheet(
    http_client: &Client,
    spreadsheet_id: &str,
    sheet_name: &str,
) -> Result<Vec<SheetTopic>, AppError> {
    let provider = google_token_provider().await?;
    let token = provider
        .token(&["https://www.googleapis.com/auth/spreadsheets.readonly"])
        .await
        .map_err(|e| AppError::GoogleAuth(format!("token fetch failed: {e}")))?;

    // Topic is in column F.
    // "already processed" flag is in column K.
    // Seed links are in column P.
    //
    // Range F2:P means:
    // F=0 G=1 H=2 I=3 J=4 K=5 L=6 M=7 N=8 O=9 P=10
    let range = sheet_range(sheet_name, "F2:P");
    let encoded_range: String = form_urlencoded::byte_serialize(range.as_bytes()).collect();
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}?majorDimension=ROWS",
        spreadsheet_id, encoded_range
    );

    let resp = http_client
        .get(&url)
        .bearer_auth(token.as_str())
        .send()
        .await
        .map_err(|e| AppError::GoogleSheets(format!("request failed: {e}")))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(AppError::GoogleSheets(format!(
            "Sheets API {}: {}",
            status, body
        )));
    }

    let parsed: GoogleValuesResponse = serde_json::from_str(&body).map_err(|e| {
        AppError::GoogleSheets(format!(
            "Failed to parse Sheets response: {} | body: {}",
            e,
            truncate(&body, 500)
        ))
    })?;

    let start_row = parsed
        .range
        .as_deref()
        .and_then(parse_start_row)
        .unwrap_or(2);

    let mut topics = Vec::new();
    for (idx, row) in parsed.values.unwrap_or_default().into_iter().enumerate() {
        let topic_raw = row.get(0).map(|s| s.as_str()).unwrap_or("");
        let topic = collapse_whitespace(topic_raw);
        let topic = topic.trim();
        if topic.is_empty() {
            continue;
        }

        // Column K (index 5 within F–P) indicates if we should skip this row.
        let processed_flag = row.get(5).map(|s| s.trim()).unwrap_or("");
        if !processed_flag.is_empty() {
            continue;
        }

        // Column P (index 10 within F–P) contains seed links (optional).
        let links_cell = row.get(10).map(|s| s.trim()).unwrap_or("");
        let seed_links = crate::utils::parse_seed_links(links_cell);

        topics.push(SheetTopic {
            row_index: start_row + idx,
            topic: topic.to_string(),
            seed_links,
        });
    }

    Ok(topics)
}

fn sheet_range(sheet_name: &str, cells: &str) -> String {
    if sheet_name.chars().any(|c| c == ' ' || c == '\'' || c == '!') {
        let escaped = sheet_name.replace('\'', "''");
        format!("'{}'!{}", escaped, cells)
    } else {
        format!("{}!{}", sheet_name, cells)
    }
}

pub async fn update_social_assets_in_google_sheet(
    http_client: &Client,
    spreadsheet_id: &str,
    sheet_name: &str,
    rows: &[SocialAssetRow],
    model_name: &str,
) -> Result<(), AppError> {
    if rows.is_empty() {
        return Ok(());
    }

    let provider = google_token_provider().await?;
    let token = provider
        .token(&["https://www.googleapis.com/auth/spreadsheets"])
        .await
        .map_err(|e| AppError::GoogleAuth(format!("token fetch failed: {e}")))?;

    let mut data = Vec::with_capacity(rows.len() * 2);

    for row in rows {
        let image_cell = if row.image_urls.is_empty() {
            String::new()
        } else {
            row.image_urls.join("\n")
        };

        let cleaned_topic = collapse_whitespace(&row.topic);
        let suggested_tags = row.suggested_tags.join(", ");

        // Update the topic in column F.
        data.push(serde_json::json!({
            "range": sheet_range(sheet_name, &format!("F{}:F{}", row.sheet_row, row.sheet_row)),
            "majorDimension": "ROWS",
            "values": [[cleaned_topic]]
        }));

        // Update slug onward starting at column J (skipping new columns G, H, I).
        data.push(serde_json::json!({
            "range": sheet_range(sheet_name, &format!("J{}:O{}", row.sheet_row, row.sheet_row)),
            "majorDimension": "ROWS",
            "values": [[
                row.slug.clone(),              // J
                row.article_title.clone(),     // K
                suggested_tags,                // L
                row.facebook_snippet.clone(),  // M
                image_cell,                    // N
                model_name                     // O
            ]]
        }));
    }

    let body = serde_json::json!({
        "valueInputOption": "USER_ENTERED",
        "data": data
    });

    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values:batchUpdate",
        spreadsheet_id
    );

    let resp = http_client
        .post(&url)
        .bearer_auth(token.as_str())
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::GoogleSheets(format!("request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::GoogleSheets(format!(
            "Sheets API {}: {}",
            status, text
        )));
    }

    Ok(())
}



fn parse_start_row(range: &str) -> Option<usize> {
    let after_bang = range.split('!').nth(1)?;
    let start_ref = after_bang.split(':').next()?;
    let digits: String = start_ref.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}
