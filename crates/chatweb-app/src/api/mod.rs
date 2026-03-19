pub mod auth;
pub mod chat;

pub fn api_base() -> String {
    let window = web_sys::window().unwrap();
    let location = window.location();
    let origin = location.origin().unwrap_or_default();
    // In production, API is on the same origin
    // In dev (trunk serve), proxy handles it
    origin
}

pub fn get_token() -> Option<String> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok()??;
    storage.get_item("authToken").ok()?
}

pub fn set_token(token: &str) {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("authToken", token);
        }
    }
}

pub fn clear_token() {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.remove_item("authToken");
        }
    }
}
