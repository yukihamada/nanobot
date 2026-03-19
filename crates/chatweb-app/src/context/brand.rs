use leptos::prelude::*;
use wasm_bindgen::JsCast;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Brand {
    Chatweb,
    Teai,
}

impl Brand {
    pub fn name(&self) -> &'static str {
        match self {
            Brand::Chatweb => "chatweb.ai",
            Brand::Teai => "teai.io",
        }
    }

    pub fn accent(&self) -> &'static str {
        match self {
            Brand::Chatweb => "#6366f1",
            Brand::Teai => "#10b981",
        }
    }

    pub fn tagline(&self) -> &'static str {
        match self {
            Brand::Chatweb => "お願いしたら、本当にやってくれるAI",
            Brand::Teai => "AI Agent for Developers",
        }
    }
}

pub fn use_brand() -> Brand {
    expect_context::<Brand>()
}

fn detect_brand() -> Brand {
    let host = web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .unwrap_or_default();
    if host.contains("teai") {
        Brand::Teai
    } else {
        Brand::Chatweb
    }
}

#[component]
pub fn BrandProvider(children: Children) -> impl IntoView {
    let brand = detect_brand();
    provide_context(brand);

    // Set CSS custom properties
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.document_element() {
            let _ = el.set_attribute(
                "data-brand",
                if brand == Brand::Teai { "teai" } else { "chatweb" },
            );
            if let Ok(html_el) = el.clone().dyn_into::<web_sys::HtmlElement>() {
                let _ = html_el.style().set_property("--brand-accent", brand.accent());
            }
        }
    }

    children()
}
