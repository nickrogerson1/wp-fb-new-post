use std::time::Duration;
use url::form_urlencoded;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::time::sleep;
use std::io::Write;

pub const BANNED_DOMAINS: [&str; 8] = [
    "allmusic.com",
    "billboard.com",
    "riaa.com",
    "imdb.com",
    "popmatters.com",
    "rockhall.com",
    "musicvf.com",
    "britannica.com"
];

pub fn is_banned_domain(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    for banned in BANNED_DOMAINS {
        if host == banned || host.ends_with(&format!(".{banned}")) {
            return true;
        }
    }
    false
}

pub fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = chars.by_ref().take(max).collect();
    format!("{}...[{} chars truncated]", truncated, s.chars().count() - max)
}

pub fn short_for_spinner(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let truncated: String = it.by_ref().take(max).collect();
    if it.next().is_some() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

pub fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_ws = false;

    for ch in input.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            in_ws = false;
            out.push(ch);
        }
    }

    out.trim().to_string()
}


pub fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

pub fn load_price(name: &str) -> Option<f64> {
    std::env::var(name).ok().and_then(|s| s.parse::<f64>().ok())
}



pub fn debug_enabled() -> bool {
    matches!(
    std::env::var("DEBUG").as_deref(),
    Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("on") | Ok("ON")
    )
}

pub fn dbg<T: std::fmt::Debug>(label: &str, val: T) {
    if debug_enabled() {
        eprintln!("[DEBUG] {label}: {val:#?}");
    }
}

pub fn dbg_plain(label: &str, value: &str) {
    eprintln!("[DEBUG] {}:\n{}", label, value);
}

// Extract a JSON object from a model response, stripping code fences if present.
pub fn extract_json(s: &str) -> Option<String> {
    let mut text = s.trim().to_string();
    if text.starts_with("```") {
        // Remove code fences
        if let Some(idx) = text.find('\n') {
            text = text[idx + 1..].to_string();
        }
        if let Some(idx) = text.rfind("```") {
            text = text[..idx].to_string();
        }
        text = text.trim().to_string();
    }
    // Try to find the first {...} block
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        return Some(text[start..=end].to_string());
    }
    None
}


pub fn encode_form_value(value: &str) -> String {
    // Create application/x-www-form-urlencoded encoding (spaces -> +, etc.)
    let mut ser = form_urlencoded::Serializer::new(String::new());
    ser.append_pair("q", value);
    let s = ser.finish(); // "q=encoded"
    s.splitn(2, '=').nth(1).unwrap_or("").to_string()
}


pub fn timer_spinner(task: impl Into<String>, item: impl Into<String>) -> Arc<AtomicBool> {
    let task = task.into();
    let item = item.into();

    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();

    tokio::spawn(async move {
        let mut secs = 0u64;
        loop {
            if done_clone.load(Ordering::Relaxed) {
                eprint!("\r{:width$}\r", "", width = 200);
                let _ = std::io::stderr().flush();
                break;
            }

            eprint!("\r{} \"{}\" {}s", task, item, secs);
            let _ = std::io::stderr().flush();
            secs += 1;
            sleep(Duration::from_secs(1)).await;
        }
    });

    done
}

pub fn normalize_emdashes(input: &str) -> String {
    input.replace('—', " - ")
}


/// Convert a spreadsheet column letter (e.g. "A", "F", "AA") into a
/// 0-based column index (e.g. 0, 5, 26).
pub fn column_letter_to_index(column: &str) -> i64 {
    column
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .fold(0i64, |acc, c| {
            acc * 26 + (c.to_ascii_uppercase() as i64 - 'A' as i64 + 1)
        })
        - 1
}

/// Convert a 0-based column index (e.g. 0, 5, 26) into a spreadsheet
/// column letter (e.g. "A", "F", "AA").
pub fn column_index_to_letter(index: i64) -> String {
    let mut n = index + 1;
    let mut letters = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        letters.push((b'A' + rem) as char);
        n = (n - 1) / 26;
    }
    letters.into_iter().rev().collect()
}

pub fn parse_seed_links(cell: &str) -> Vec<String> {
    use url::Url;

    let raw = cell.trim();
    if raw.is_empty() {
        return vec![];
    }

    raw.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '|')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        // strip common surrounding/trailing punctuation from spreadsheets
        .map(|s| s.trim_matches(|c: char| c == '(' || c == ')' || c == '[' || c == ']' || c == '"' ))
        .map(|s| s.trim_end_matches(|c: char| ".!?)]}\"'".contains(c)))
        // keep only valid absolute URLs
        .filter_map(|s| Url::parse(s).ok().map(|u| u.to_string()))
        .collect()
}