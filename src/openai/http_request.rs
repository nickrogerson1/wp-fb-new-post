use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use reqwest::{redirect::Policy, Client, Method};
use url::Url;

use crate::{
    errors::AppError,
    models::UsageTotals,
    utils::{dbg, truncate, is_banned_domain},
};

const MAX_REDIRECTS: usize = 5;
const CHAT_COMPLETIONS_URL: &str = "https://api.openai.com/v1/chat/completions";

pub async fn post_openai_http(
    client: &Client,
    openai_key: &str,
    mut payload: Value,
    usage_totals: Arc<UsageTotals>,
    usage_label: &str,
) -> Result<Value, AppError> {
    loop {
        // dbg(
        //     &format!("{} payload", usage_label),
        //     serde_json::to_string_pretty(&payload).unwrap_or_default(),
        // );

        let resp = client
            .post(CHAT_COMPLETIONS_URL)
            .bearer_auth(openai_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                dbg(&format!("{} reqwest send error", usage_label), &e);
                AppError::Http(e.to_string())
            })?;

        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        // dbg(
        //     &format!("{} raw response (truncated)", usage_label),
        //     truncate(&raw, 4000),
        // );

        if !status.is_success() {
            return Err(AppError::Http(format!(
                "OpenAI error {}: {}",
                status, raw
            )));
        }

        let v: Value = serde_json::from_str(&raw).map_err(|e| {
            dbg(&format!("{} JSON parse error", usage_label), &e);
            AppError::Http(format!(
                "Failed to parse OpenAI JSON: {} | body: {}",
                e,
                truncate(&raw, 2000)
            ))
        })?;

        if let Some(usage) = v.get("usage") {
            let input = usage["prompt_tokens"].as_u64().unwrap_or(0);
            let output = usage["completion_tokens"].as_u64().unwrap_or(0);
            println!(
                "[TOKENS][{}] input: {}, output: {}, total: {}",
                usage_label,
                input,
                output,
                input + output
            );
            usage_totals.add(input, output);
        }

        // Look at the first choice
        let Some(choice) = v["choices"].as_array().and_then(|arr| arr.first()) else {
            return Err(AppError::EmptyOpenAI);
        };
        let message = &choice["message"];

        // If there are tool calls, handle each and loop again.
        if let Some(tool_calls) = message.get("tool_calls").and_then(|tc| tc.as_array()) {

            // We need to append both the assistant's tool-call message and our tool responses
            let messages = payload
                .get_mut("messages")
                .and_then(|m| m.as_array_mut())
                .ok_or_else(|| AppError::Http("payload missing messages array".into()))?;

            // Record the assistant tool-call turn in the conversation history
            messages.push(message.clone());

            for tool_call in tool_calls {
                let tool_call_id = tool_call["id"].as_str().unwrap_or_default();
                let name = tool_call["function"]["name"].as_str().unwrap_or_default();
                let args_str = tool_call["function"]["arguments"].as_str().unwrap_or("{}");
                let args_json: Value = serde_json::from_str(args_str).unwrap_or(Value::Null);

                // Short log entry – shows which tool and the key arguments (url/method).
                if name == "httpHead" {
                    let url = args_json.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let method = args_json.get("method").and_then(|v| v.as_str()).unwrap_or("HEAD");
                    eprintln!("[TOOL] {name} {method} {url}");
                }

                let response_json = match name {
                    "httpHead" => handle_http_head_call(&args_json).await?,
                    other => {
                        return Err(AppError::Tool(format!("Unsupported tool: {}", other)));
                    }
                };

                // Push our tool response as a new message
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": serde_json::to_string(&response_json).unwrap_or_default()
                }));
            }

            // Loop again so OpenAI can continue with the tool output.
            continue;
        }

        // No tool calls: return the entire response Value to caller.
        return Ok(v);
    }
}


async fn handle_http_head_call(
    args: &serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let original_url = args["url"]
        .as_str()
        .ok_or_else(|| AppError::Tool("httpHead missing url".into()))?;
    let method = args
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("HEAD")
        .to_uppercase();

    let method_enum = match method.as_str() {
        "GET" => Method::GET,
        _ => Method::HEAD,
    };

    let base_url = Url::parse(original_url)
        .map_err(|e| AppError::Tool(format!("Invalid URL {original_url}: {e}")))?;
    let base_host = base_url.host_str().map(|s| s.to_string());

    if let Some(ref host) = base_host {
        if is_banned_domain(host) {
            eprintln!(
                "\x1b[31m[TOOL][httpHead] blocked domain {} for {}\x1b[0m",
                host, original_url
            );
            return Ok(json!({
                "url": original_url,
                "finalUrl": original_url,
                "status": null,
                "ok": false,
                "bodyPreview": "",
                "redirects": [],
                "error": "Domain is on the banned list"
            }));
        }
    }

    let mut current_url = original_url.to_string();
    let mut redirects = Vec::new();

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(Policy::none())
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".parse().unwrap());
            headers
        })
        .build()
        .map_err(|e| AppError::Http(format!("httpHead client init failed: {e}")))?;

    for hop in 0..=MAX_REDIRECTS {
        let response = http
            .request(method_enum.clone(), &current_url)
            .send()
            .await;

        let resp = match response {
            Ok(resp) => resp,
            Err(err) => {
                eprintln!(
                    "\x1b[31m[TOOL][httpHead] {} {} -> error: {}\x1b[0m",
                    method, current_url, err
                );
                return Ok(json!({
                    "url": original_url,
                    "finalUrl": current_url,
                    "status": null,
                    "ok": false,
                    "bodyPreview": "",
                    "redirects": redirects,
                    "error": err.to_string()
                }));
            }
        };

        let status = resp.status();
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|loc| loc.to_str().ok())
            .map(|s| s.to_string());

        let body = if method_enum == Method::GET || !status.is_success() {
            resp.text().await.unwrap_or_default()
        } else {
            String::new()
        };

        if status.is_success() {
            eprintln!(
                "\x1b[32m[TOOL][httpHead] {} {} -> {}\x1b[0m",
                method, current_url, status
            );
            return Ok(json!({
                "url": original_url,
                "finalUrl": current_url,
                "status": status.as_u16(),
                "ok": true,
                "bodyPreview": truncate(&body, 100),
                "redirects": redirects,
                "error": null
            }));
        }

        if status.is_redirection() {
            let Some(location_value) = location.clone() else {
                eprintln!(
                    "\x1b[31m[TOOL][httpHead] {} {} -> {} (missing Location header)\x1b[0m",
                    method, current_url, status
                );
                return Ok(json!({
                    "url": original_url,
                    "finalUrl": current_url,
                    "status": status.as_u16(),
                    "ok": false,
                    "bodyPreview": truncate(&body, 100),
                    "redirects": redirects,
                    "error": format!("Redirected with status {} but no Location header", status)
                }));
            };

            let next_url = Url::parse(&current_url)
                .ok()
                .and_then(|base| base.join(&location_value).ok())
                .map(|u| u.to_string());

            let Some(next_url_str) = next_url else {
                eprintln!(
                    "\x1b[31m[TOOL][httpHead] {} {} -> {} (bad redirect target {})\x1b[0m",
                    method, current_url, status, location_value
                );
                return Ok(json!({
                    "url": original_url,
                    "finalUrl": current_url,
                    "status": status.as_u16(),
                    "ok": false,
                    "bodyPreview": truncate(&body, 100),
                    "redirects": redirects,
                    "error": format!("Redirected {} but target '{}' could not be resolved", status, location_value)
                }));
            };

            let next_url_parsed = Url::parse(&next_url_str).ok();
            let next_host = next_url_parsed
                .as_ref()
                .and_then(|u| u.host_str().map(|s| s.to_string()));

            if let Some(ref host) = next_host {
                if is_banned_domain(host) {
                    eprintln!(
                        "\x1b[31m[TOOL][httpHead] blocked redirect domain {} for {}\x1b[0m",
                        host, next_url_str
                    );
                    return Ok(json!({
                        "url": original_url,
                        "finalUrl": current_url,
                        "status": status.as_u16(),
                        "ok": false,
                        "bodyPreview": truncate(&body, 100),
                        "redirects": redirects,
                        "error": "Redirected to banned domain"
                    }));
                }
            }

            if base_host.is_some() && next_host.is_some() && next_host != base_host {
                eprintln!(
                    "\x1b[31m[TOOL][httpHead] {} {} -> {} (cross-domain redirect to {})\x1b[0m",
                    method, current_url, status, next_url_str
                );
                return Ok(json!({
                    "url": original_url,
                    "finalUrl": current_url,
                    "status": status.as_u16(),
                    "ok": false,
                    "bodyPreview": truncate(&body, 100),
                    "redirects": redirects,
                    "error": format!("Redirected from {} to different domain {}", current_url, next_url_str)
                }));
            }

            let next_path = next_url_parsed
                .as_ref()
                .map(|u| u.path())
                .unwrap_or("");
            if next_path.is_empty() || next_path == "/" {
                eprintln!(
                    "\x1b[31m[TOOL][httpHead] {} {} -> {} (redirected to homepage {})\x1b[0m",
                    method, current_url, status, next_url_str
                );
                return Ok(json!({
                    "url": original_url,
                    "finalUrl": next_url_str,
                    "status": status.as_u16(),
                    "ok": false,
                    "bodyPreview": truncate(&body, 100),
                    "redirects": redirects,
                    "error": "Redirected to site homepage; treat as invalid source."
                }));
            }

            eprintln!(
                "\x1b[33m[TOOL][httpHead] {} {} -> {} (redirect to {})\x1b[0m",
                method, current_url, status, next_url_str
            );

            redirects.push(json!({
                "status": status.as_u16(),
                "from": current_url,
                "to": next_url_str
            }));
            current_url = next_url_str;

            if hop == MAX_REDIRECTS {
                return Ok(json!({
                    "url": original_url,
                    "finalUrl": current_url,
                    "status": status.as_u16(),
                    "ok": false,
                    "bodyPreview": truncate(&body, 100),
                    "redirects": redirects,
                    "error": format!("Exceeded {MAX_REDIRECTS} redirects without reaching a 200")
                }));
            }

            continue;
        }

        if status.as_u16() == 403 {
            eprintln!(
                "\x1b[34m[TOOL][httpHead] {} {} -> {} (forbidden)\x1b[0m",
                method,
                current_url,
                status
            );
            return Ok(json!({
                "url": original_url,
                "finalUrl": current_url,
                "status": status.as_u16(),
                "ok": false,
                "bodyPreview": truncate(&body, 100),
                "redirects": redirects,
                "error": "403 Forbidden"
            }));
        }

        // Existing catch-all for other errors
        eprintln!(
            "\x1b[31m[TOOL][httpHead] {} {} -> {} | {}\x1b[0m",
            method,
            current_url,
            status,
            truncate(&body, 200)
        );
        return Ok(json!({
            "url": original_url,
            "finalUrl": current_url,
            "status": status.as_u16(),
            "ok": false,
            "bodyPreview": truncate(&body, 1000),
            "redirects": redirects,
            "error": format!("Status {}: {}", status, truncate(&body, 200))
        }));
    }

    unreachable!("redirect loop should return before here");
}