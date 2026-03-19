use leptos::prelude::*;
use wasm_bindgen_futures;
use crate::api::auth::{fetch_me, AuthMe};

#[derive(Clone, Debug)]
pub struct AuthState {
    pub authenticated: RwSignal<bool>,
    pub user_id: RwSignal<Option<String>>,
    pub display_name: RwSignal<Option<String>>,
    pub email: RwSignal<Option<String>>,
    pub plan: RwSignal<String>,
    pub credits_remaining: RwSignal<i64>,
}

impl AuthState {
    pub fn new() -> Self {
        Self {
            authenticated: RwSignal::new(false),
            user_id: RwSignal::new(None),
            display_name: RwSignal::new(None),
            email: RwSignal::new(None),
            plan: RwSignal::new("free".to_string()),
            credits_remaining: RwSignal::new(0),
        }
    }

    pub fn update_from(&self, me: &AuthMe) {
        self.authenticated.set(me.authenticated);
        self.user_id.set(me.user_id.clone());
        self.display_name.set(me.display_name.clone());
        self.email.set(me.email.clone());
        self.plan.set(me.plan.clone().unwrap_or_else(|| "free".to_string()));
        self.credits_remaining.set(me.credits_remaining.unwrap_or(0));
    }

    pub fn clear(&self) {
        self.authenticated.set(false);
        self.user_id.set(None);
        self.display_name.set(None);
        self.email.set(None);
        self.plan.set("free".to_string());
        self.credits_remaining.set(0);
    }
}

pub fn use_auth() -> AuthState {
    expect_context::<AuthState>()
}

/// Extract a query parameter value from the current URL
fn get_url_param(name: &str) -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get(name)
}

/// Remove auth-related query params from the URL (clean up after OAuth redirect)
fn clean_auth_url_params() {
    if let Some(window) = web_sys::window() {
        if let Ok(href) = window.location().href() {
            if let Ok(url) = web_sys::Url::new(&href) {
                let params = url.search_params();
                params.delete("auth");
                params.delete("token");
                params.delete("reason");
                let new_url = format!("{}{}", url.origin(), url.pathname());
                let _ = window.history()
                    .and_then(|h| h.replace_state_with_url(
                        &wasm_bindgen::JsValue::NULL,
                        "",
                        Some(&new_url),
                    ));
            }
        }
    }
}

#[component]
pub fn AuthProvider(children: Children) -> impl IntoView {
    let state = AuthState::new();
    provide_context(state.clone());

    // Check for OAuth redirect token in URL params
    if let Some(token) = get_url_param("token") {
        if get_url_param("auth").as_deref() == Some("success") && !token.is_empty() {
            crate::api::set_token(&token);
            clean_auth_url_params();
        }
    } else {
        clean_auth_url_params();
    }

    // Check auth on mount
    let state_clone = state.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(me) = fetch_me().await {
            state_clone.update_from(&me);
        }
    });

    children()
}
