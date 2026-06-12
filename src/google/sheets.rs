use reqwest::Client;
use url::form_urlencoded;
use crate::{
    google::auth::google_token_provider,
    errors::AppError,
    config::SheetLayout,
    utils::{collapse_whitespace, truncate, column_letter_to_index, column_index_to_letter},
    models::SheetTopic,
    models::SocialAssetRow,
    models::GoogleValuesResponse
};

pub async fn load_topics_from_sheet(
    http_client: &Client,
    spreadsheet_id: &str,
    sheet_name: &str,
    layout: &SheetLayout,
) -> Result<Vec<SheetTopic>, AppError> {
    let provider = google_token_provider().await?;
    let token = provider
        .token(&["https://www.googleapis.com/auth/spreadsheets.readonly"])
        .await
        .map_err(|e| AppError::GoogleAuth(format!("token fetch failed: {e}")))?;

    // Topic, "already processed" flag, and seed links columns are configurable
    // via SHEET_TOPIC_COLUMN / SHEET_PROCESSED_COLUMN / SHEET_SEED_LINKS_COLUMN.
    // Read the smallest range that spans all three, then work out each
    // column's offset within that range.
    let topic_idx = column_letter_to_index(&layout.topic_column);
    let processed_idx = column_letter_to_index(&layout.processed_column);
    let seed_links_idx = column_letter_to_index(&layout.seed_links_column);

    let min_idx = topic_idx.min(processed_idx).min(seed_links_idx);
    let max_idx = topic_idx.max(processed_idx).max(seed_links_idx);

    let start_col = column_index_to_letter(min_idx);
    let end_col = column_index_to_letter(max_idx);

    let topic_offset = (topic_idx - min_idx) as usize;
    let processed_offset = (processed_idx - min_idx) as usize;
    let seed_links_offset = (seed_links_idx - min_idx) as usize;

    let range = sheet_range(sheet_name, &format!("{start_col}2:{end_col}"));
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
        let topic_raw = row.get(topic_offset).map(|s| s.as_str()).unwrap_or("");
        let topic = collapse_whitespace(topic_raw);
        let topic = topic.trim();
        if topic.is_empty() {
            continue;
        }

        // The "already processed" flag indicates if we should skip this row.
        let processed_flag = row.get(processed_offset).map(|s| s.trim()).unwrap_or("");
        if !processed_flag.is_empty() {
            continue;
        }

        // Seed links column (optional).
        let links_cell = row.get(seed_links_offset).map(|s| s.trim()).unwrap_or("");
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
    layout: &SheetLayout,
) -> Result<(), AppError> {
    if rows.is_empty() {
        return Ok(());
    }

    let provider = google_token_provider().await?;
    let token = provider
        .token(&["https://www.googleapis.com/auth/spreadsheets"])
        .await
        .map_err(|e| AppError::GoogleAuth(format!("token fetch failed: {e}")))?;

    let output_start_idx = column_letter_to_index(&layout.output_column);
    let output_end_col = column_index_to_letter(output_start_idx + 5);

    let mut data = Vec::with_capacity(rows.len() * 2);

    for row in rows {
        let image_cell = if row.image_urls.is_empty() {
            String::new()
        } else {
            row.image_urls.join("\n")
        };

        let cleaned_topic = collapse_whitespace(&row.topic);
        let suggested_tags = row.suggested_tags.join(", ");

        // Update the topic in the topic column.
        data.push(serde_json::json!({
            "range": sheet_range(sheet_name, &format!("{col}{row}:{col}{row}", col = layout.topic_column, row = row.sheet_row)),
            "majorDimension": "ROWS",
            "values": [[cleaned_topic]]
        }));

        // Update slug onward starting at the output column (a 6-column block:
        // slug, title, tags, Facebook snippet, image URLs, model).
        data.push(serde_json::json!({
            "range": sheet_range(sheet_name, &format!("{start}{row}:{end}{row}", start = layout.output_column, end = output_end_col, row = row.sheet_row)),
            "majorDimension": "ROWS",
            "values": [[
                row.slug.clone(),
                row.article_title.clone(),
                suggested_tags,
                row.facebook_snippet.clone(),
                image_cell,
                model_name
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
