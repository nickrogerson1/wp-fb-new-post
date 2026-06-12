use {
    crate::{
        errors::AppError,
        utils::truncate,
        models::UsageTotals,
    },
    std::sync::Arc,
};

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";


// pub async fn post_gemini_http(
//     http_client: &reqwest::Client,
//     api_key: &str,
//     model: &str,
//     mut body: serde_json::Value,
//     usage_totals: &Arc<UsageTotals>,
//     stage: &str,
// ) -> Result<String, AppError> {
//     // We'll keep reusing/augmenting `body["contents"]`.
//     loop {
//         let response = http_client
//             .post(format!("{BASE_URL}/models/{}:generateContent", model))
//             .header("x-goog-api-key", api_key)
//             .json(&body)
//             .send()
//             .await
//             .map_err(|e| AppError::Http(format!("Gemini request failed: {e}")))?;

//         let status = response.status();
//         let raw = response.text().await.unwrap_or_default();
//         if !status.is_success() {
//             return Err(AppError::Http(format!(
//                 "Gemini error {}: {}",
//                 status,
//                 truncate(&raw, 2000)
//             )));
//         }

//         let v: serde_json::Value = serde_json::from_str(&raw).map_err(AppError::Json)?;

//         if let Some(usage) = v.get("usageMetadata") {
//             let input = usage["promptTokenCount"].as_u64().unwrap_or(0);
//             let output = usage["candidatesTokenCount"].as_u64().unwrap_or(0);
//             println!("[TOKENS][gemini][{stage}] input: {input}, output: {output}");
//             usage_totals.add(input, output);
//         }

//         // Look at the first candidate
//         let candidate = match v["candidates"].as_array().and_then(|arr| arr.first()) {
//             Some(c) => c,
//             None => return Err(AppError::EmptyGemini),
//         };

//         // 1) If Gemini returned text, extract and return it.
//         if let Some(parts) = candidate["content"]["parts"].as_array() {
//             if let Some(text_part) =
//                 parts.iter().find(|p| p.get("text").is_some())
//             {
//                 let text = text_part["text"].as_str().unwrap_or("").trim().to_string();
//                 if !text.is_empty() {
//                     return Ok(text);
//                 }
//             }
//         }

//         // 2) Otherwise, see if Gemini issued a tool/function call.
//         if let Some(parts) = candidate["content"]["parts"].as_array() {
//             if let Some(call_part) = parts.iter().find(|p| p.get("functionCall").is_some()) {
//                 let function_call = &call_part["functionCall"];
//                 let name = function_call["name"].as_str().unwrap_or("");
//                 let args = &function_call["args"];

//                 if name == "httpHead" {
//                     // Run your Rust handler.
//                     let tool_response = handle_http_head_call(http_client, args).await?;

//                     // Append Gemini's function call + your tool response to the conversation.
//                     // The conversation is body["contents"], an array of role/content pairs.
//                     let contents = body["contents"].as_array_mut().expect("contents array");

//                     // 2a) Add Gemini’s function call turn (role=“model”)
//                     contents.push(json!({
//                         "role": "model",
//                         "parts": [ { "functionCall": { "name": name, "args": args } } ]
//                     }));

//                     // 2b) Add tool response (role=“function”).
//                     contents.push(json!({
//                         "role": "function",
//                         "parts": [ { "functionResponse": {
//                             "name": name,
//                             "response": tool_response
//                         }}]
//                     }));

//                     // Continue the loop; Gemini will see the tool output and keep generating.
//                     continue;
//                 }
//             }
//         }

//         // If we didn’t return text and didn’t handle a tool call, treat as an error.
//         return Err(AppError::EmptyGemini);
//     }
// }



// async fn handle_http_head_call(
//     client: &reqwest::Client,
//     args: &serde_json::Value,
// ) -> Result<serde_json::Value, AppError> {
//     let url = args["url"].as_str().ok_or_else(|| AppError::Tool("missing url".into()))?;
//     let method = args.get("method").and_then(|m| m.as_str()).unwrap_or("HEAD");

//     let request = match method {
//         "GET" => client.get(url),
//         _ => client.head(url),
//     };

//     let resp = request
//         .timeout(Duration::from_secs(10))
//         .send()
//         .await
//         .map_err(|e| AppError::Http(format!("httpHead request failed: {}", e)))?;

//     let status = resp.status();
//     let body_preview = if method == "GET" || !status.is_success() {
//         truncate(&resp.text().await.unwrap_or_default(), 1000).to_string()
//     } else {
//         String::new()
//     };

//     Ok(serde_json::json!({
//         "url": url,
//         "status": status.as_u16(),
//         "ok": status.is_success(),
//         "bodyPreview": body_preview
//     }))
// }


pub async fn post_gemini_http(
    http_client: &reqwest::Client,
    api_key: &str,
    model: &str,
    body: serde_json::Value,
    usage_totals: &Arc<UsageTotals>,
    stage: &str,
) -> Result<String, AppError> {

    let url = format!(
        "{}/models/{}:generateContent",
        BASE_URL, model
    );

    let resp = http_client
        .post(url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Http(format!("Gemini request failed: {e}")))?;

    let status = resp.status();
    let raw = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(AppError::Http(format!(
            "Gemini error {}: {}",
            status,
            truncate(&raw, 2000)
        )));
    }

    let v: serde_json::Value = serde_json::from_str(&raw).map_err(AppError::Json)?;

    if let Some(usage) = v.get("usageMetadata") {
        let input = usage.get("promptTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let output = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("[TOKENS][gemini][{stage}] input: {input}, output: {output}");
        usage_totals.add(input, output);
    }

    let text = v["candidates"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|cand| cand["content"]["parts"].as_array())
        .and_then(|parts| parts.iter().find_map(|p| p["text"].as_str()))
        .unwrap_or("")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err(AppError::EmptyGemini);
    }

    Ok(text)
}