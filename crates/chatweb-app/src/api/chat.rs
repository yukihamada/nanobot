use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Response};

use super::{api_base, get_token};
use crate::types::chat::{ChatRequest, SseEvent};

/// Stream chat via SSE. Calls `on_event` for each parsed SSE event.
pub async fn stream_chat(
    req: &ChatRequest,
    on_event: impl Fn(SseEvent) + 'static,
) -> Result<(), String> {
    let url = format!("{}/api/v1/chat/stream", api_base());
    let body = serde_json::to_string(req).map_err(|e| e.to_string())?;

    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(RequestMode::Cors);

    let headers = web_sys::Headers::new().map_err(|e| format!("{:?}", e))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("{:?}", e))?;
    if let Some(token) = get_token() {
        headers.set("Authorization", &format!("Bearer {}", token)).map_err(|e| format!("{:?}", e))?;
    }
    opts.set_headers(&headers);
    opts.set_body(&JsValue::from_str(&body));

    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: Response = resp_value.dyn_into().map_err(|_| "not a Response")?;

    if !resp.ok() {
        let status = resp.status();
        let text = JsFuture::from(resp.text().map_err(|e| format!("{:?}", e))?)
            .await
            .map_err(|e| format!("{:?}", e))?
            .as_string()
            .unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, text));
    }

    let body = resp.body().ok_or("no body")?;
    let reader = body.get_reader();
    let reader: web_sys::ReadableStreamDefaultReader = reader.dyn_into().map_err(|_| "not a reader")?;
    let decoder = js_sys::eval("new TextDecoder()").map_err(|e| format!("{:?}", e))?;

    let mut buffer = String::new();

    loop {
        let result = JsFuture::from(reader.read())
            .await
            .map_err(|e| format!("{:?}", e))?;
        let done = js_sys::Reflect::get(&result, &JsValue::from_str("done"))
            .map_err(|e| format!("{:?}", e))?
            .as_bool()
            .unwrap_or(true);
        if done {
            break;
        }
        let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))
            .map_err(|e| format!("{:?}", e))?;

        // Decode Uint8Array to string
        let decode_fn = js_sys::Reflect::get(&decoder, &JsValue::from_str("decode"))
            .map_err(|e| format!("{:?}", e))?;
        let decode_fn: js_sys::Function = decode_fn.dyn_into().map_err(|_| "not a function")?;
        let chunk_str = decode_fn
            .call1(&decoder, &value)
            .map_err(|e| format!("{:?}", e))?
            .as_string()
            .unwrap_or_default();

        buffer.push_str(&chunk_str);

        // Parse complete SSE lines
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim_end().to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.starts_with("data: ") {
                let data = &line[6..];
                // Try parsing as JSON — handle both single object and array
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(arr) = parsed.as_array() {
                        for item in arr {
                            if let Ok(evt) = serde_json::from_value::<SseEvent>(item.clone()) {
                                on_event(evt);
                            }
                        }
                    } else if let Ok(evt) = serde_json::from_value::<SseEvent>(parsed) {
                        on_event(evt);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Fetch conversation list
pub async fn fetch_conversations() -> Result<Vec<crate::types::chat::Conversation>, String> {
    let url = format!("{}/api/v1/conversations", api_base());
    let opts = RequestInit::new();
    opts.set_method("GET");
    if let Some(token) = get_token() {
        let headers = web_sys::Headers::new().map_err(|e| format!("{:?}", e))?;
        headers.set("Authorization", &format!("Bearer {}", token)).map_err(|e| format!("{:?}", e))?;
        opts.set_headers(&headers);
    }
    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: Response = resp_value.dyn_into().map_err(|_| "not a Response")?;
    if !resp.ok() {
        return Ok(vec![]);
    }
    let json = JsFuture::from(resp.json().map_err(|e| format!("{:?}", e))?)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let convs: Vec<crate::types::chat::Conversation> =
        serde_wasm_bindgen::from_value(json).unwrap_or_default();
    Ok(convs)
}
