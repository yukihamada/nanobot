use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};

use super::{api_base, get_token};

#[derive(Clone, Debug, Deserialize)]
pub struct AuthMe {
    pub authenticated: bool,
    pub user_id: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub credits_remaining: Option<i64>,
    pub credits_used: Option<i64>,
}

pub async fn fetch_me() -> Result<AuthMe, String> {
    let url = format!("{}/api/v1/auth/me", api_base());
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
    let json = JsFuture::from(resp.json().map_err(|e| format!("{:?}", e))?)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let me: AuthMe = serde_wasm_bindgen::from_value(json).map_err(|e| format!("{:?}", e))?;
    Ok(me)
}

pub async fn login_email(email: &str, password: &str) -> Result<String, String> {
    let url = format!("{}/api/v1/auth/login", api_base());
    let body = serde_json::json!({ "email": email, "password": password });
    let opts = RequestInit::new();
    opts.set_method("POST");
    let headers = web_sys::Headers::new().map_err(|e| format!("{:?}", e))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("{:?}", e))?;
    opts.set_headers(&headers);
    opts.set_body(&wasm_bindgen::JsValue::from_str(&body.to_string()));

    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: Response = resp_value.dyn_into().map_err(|_| "not a Response")?;
    let json = JsFuture::from(resp.json().map_err(|e| format!("{:?}", e))?)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let obj: serde_json::Value = serde_wasm_bindgen::from_value(json).map_err(|e| format!("{:?}", e))?;

    if let Some(token) = obj.get("token").and_then(|t| t.as_str()) {
        super::set_token(token);
        Ok(token.to_string())
    } else if let Some(err) = obj.get("error").and_then(|e| e.as_str()) {
        Err(err.to_string())
    } else {
        Err("Login failed".to_string())
    }
}

pub async fn register_email(email: &str, password: &str, display_name: &str) -> Result<String, String> {
    let url = format!("{}/api/v1/auth/register", api_base());
    let body = serde_json::json!({
        "email": email,
        "password": password,
        "display_name": display_name,
    });
    let opts = RequestInit::new();
    opts.set_method("POST");
    let headers = web_sys::Headers::new().map_err(|e| format!("{:?}", e))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("{:?}", e))?;
    opts.set_headers(&headers);
    opts.set_body(&wasm_bindgen::JsValue::from_str(&body.to_string()));

    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: Response = resp_value.dyn_into().map_err(|_| "not a Response")?;
    let json = JsFuture::from(resp.json().map_err(|e| format!("{:?}", e))?)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let obj: serde_json::Value = serde_wasm_bindgen::from_value(json).map_err(|e| format!("{:?}", e))?;

    if let Some(token) = obj.get("token").and_then(|t| t.as_str()) {
        super::set_token(token);
        Ok(token.to_string())
    } else if let Some(err) = obj.get("error").and_then(|e| e.as_str()) {
        Err(err.to_string())
    } else {
        Err("Registration failed".to_string())
    }
}

/// Passwordless email auth result
#[derive(Clone, Debug, Deserialize)]
pub struct EmailAuthResponse {
    pub ok: Option<bool>,
    pub pending_verification: Option<bool>,
    pub token: Option<String>,
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub message: Option<String>,
    pub error: Option<String>,
}

/// POST /api/v1/auth/email — send verification code or instant auth
pub async fn auth_email(email: &str) -> Result<EmailAuthResponse, String> {
    let url = format!("{}/api/v1/auth/email", api_base());
    let body = serde_json::json!({ "email": email });
    let opts = RequestInit::new();
    opts.set_method("POST");
    let headers = web_sys::Headers::new().map_err(|e| format!("{:?}", e))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("{:?}", e))?;
    opts.set_headers(&headers);
    opts.set_body(&wasm_bindgen::JsValue::from_str(&body.to_string()));

    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: Response = resp_value.dyn_into().map_err(|_| "not a Response")?;
    let json = JsFuture::from(resp.json().map_err(|e| format!("{:?}", e))?)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let res: EmailAuthResponse = serde_wasm_bindgen::from_value(json).map_err(|e| format!("{:?}", e))?;

    // If instant auth returned a token, save it
    if let Some(ref token) = res.token {
        super::set_token(token);
    }
    Ok(res)
}

/// POST /api/v1/auth/verify — verify 6-digit code
pub async fn verify_code(email: &str, code: &str) -> Result<EmailAuthResponse, String> {
    let url = format!("{}/api/v1/auth/verify", api_base());
    let body = serde_json::json!({ "email": email, "code": code });
    let opts = RequestInit::new();
    opts.set_method("POST");
    let headers = web_sys::Headers::new().map_err(|e| format!("{:?}", e))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("{:?}", e))?;
    opts.set_headers(&headers);
    opts.set_body(&wasm_bindgen::JsValue::from_str(&body.to_string()));

    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: Response = resp_value.dyn_into().map_err(|_| "not a Response")?;
    let json = JsFuture::from(resp.json().map_err(|e| format!("{:?}", e))?)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let res: EmailAuthResponse = serde_wasm_bindgen::from_value(json).map_err(|e| format!("{:?}", e))?;

    if let Some(ref token) = res.token {
        super::set_token(token);
    }
    Ok(res)
}
