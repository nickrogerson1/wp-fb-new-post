use rust_xlsxwriter::{Format, FormatAlign, Color, Workbook, XlsxError};
use chrono::Local;
use url::Url;
use sha2::{Digest, Sha256};
use crate::{
    errors::AppError,
    models::SocialAssetRow,
    utils::encode_form_value
};

pub fn write_social_assets_xlsx(rows: &[SocialAssetRow]) -> Result<(), AppError> {
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();

    worksheet.set_freeze_panes(1, 0).map_err(xlsx_err)?;

    worksheet.set_column_width(0, 28.0).map_err(xlsx_err)?; // Topic
    worksheet.set_column_width(1, 40.0).map_err(xlsx_err)?; // Title
    worksheet.set_column_width(2, 40.0).map_err(xlsx_err)?; // Suggested Tags
    worksheet.set_column_width(3, 65.0).map_err(xlsx_err)?; // Snippet (wide)
    worksheet.set_column_width(4, 120.0).map_err(xlsx_err)?; // Image URLs

    let header_format = Format::new()
        .set_bold()
        .set_align(FormatAlign::Center)
        .set_background_color(Color::RGB(0xF2F2F2));

    let wrap_format = Format::new().set_text_wrap();
    let snippet_format = Format::new().set_text_wrap();
    let image_format = Format::new().set_text_wrap();

    let headers = ["Topic", "Article Title", "Suggested Tags", "Facebook Snippet", "Image URLs"];
    for (col, header) in headers.iter().enumerate() {
        worksheet
            .write_string_with_format(0, col as u16, *header, &header_format)
            .map_err(xlsx_err)?;
    }

    for (idx, row) in rows.iter().enumerate() {
        let r = (idx + 1) as u32;
        let tags_cell = row.suggested_tags.join(", ");
        worksheet
            .write_string_with_format(r, 0, &row.topic, &wrap_format)
            .map_err(xlsx_err)?;
        worksheet
            .write_string_with_format(r, 1, &row.article_title, &wrap_format)
            .map_err(xlsx_err)?;
        worksheet
            .write_string_with_format(r, 2, &tags_cell, &wrap_format)
            .map_err(xlsx_err)?;
        worksheet
            .write_string_with_format(r, 3, &row.facebook_snippet, &snippet_format)
            .map_err(xlsx_err)?;

        let image_cell = if row.image_urls.is_empty() {
            String::new()
        } else {
            row.image_urls.join("\n")
        };
        worksheet
            .write_string_with_format(r, 4, &image_cell, &image_format)
            .map_err(xlsx_err)?;
    }

    // workbook.save("social_assets.xlsx").map_err(xlsx_err)?;
     // Ensure the `social_assets` directory exists
    let out_dir = std::path::Path::new("social_assets");
    if !out_dir.exists() {
        std::fs::create_dir_all(out_dir)?;
    }

    // Timestamp like 20251123_154530
    let now = Local::now();
    let timestamp = now.format("%Y%m%d_%H%M%S").to_string();

    let filename = format!("social_assets_{}.xlsx", timestamp);
    let out_path = out_dir.join(filename);

    workbook.save(&out_path).map_err(xlsx_err)?;
    Ok(())
}


pub fn sanitize_filename(s: &str) -> String {
    let mut slug = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() || c == '-' || c == '_' {
            slug.push('-');
        }
    }
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_string();

    // ensure non-empty and short enough for Windows
    let mut final_slug = if slug.is_empty() {
        "topic".to_string()
    } else {
        slug
    };

    const MAX_LEN: usize = 80;
    if final_slug.len() > MAX_LEN {
        // append 8 hex chars from a hash to keep it unique
        let mut hasher = Sha256::new();
        hasher.update(s.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        final_slug.truncate(MAX_LEN);
        final_slug.push('-');
        final_slug.push_str(&hash[..8]);
    }

    final_slug
}


pub fn short_slug_from_title(title: &str) -> String {
    let mut words = title
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase());

    let mut slug_words = Vec::new();
    for _ in 0..5 {
        if let Some(word) = words.next() {
            slug_words.push(word);
        } else {
            break;
        }
    }

    // Guarantee at least four words when possible
    if slug_words.len() > 4 {
        slug_words.truncate(5);
    }

    slug_words.join("-")
}


pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}



pub fn upgrade_to_google_advanced(u: &str) -> Option<String> {
    let parsed = Url::parse(u).ok()?;
    let domain = parsed.domain()?;
    if !domain.ends_with("google.com") {
    return None;
    }
    // Accept common Google image/search paths
    let p = parsed.path();
    if p != "/search" && p != "/images" && p != "/imgres" {
    return None;
    }

    // Grab an existing query from either q or as_q
    let mut q_val: Option<String> = None;
    for (k, v) in parsed.query_pairs() {
        if (k == "as_q" || k == "q") && !v.is_empty() {
            q_val = Some(v.into_owned());
            break;
        }
    }

    let q = q_val?;
    let encoded_q = encode_form_value(&q);

    Some(format!(
        "https://www.google.com/search?as_st=y&as_q={}&as_epq=&as_oq=&as_eq=&imgsz=l&imgar=w&imgcolor=&imgtype=&cr=&as_sitesearch=&as_filetype=&tbs=&udm=2",
        encoded_q
    ))
}



pub fn xlsx_err(e: XlsxError) -> AppError {
    AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}