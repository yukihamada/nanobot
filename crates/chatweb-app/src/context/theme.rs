use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn as_str(&self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

pub fn use_theme() -> ReadSignal<Theme> {
    let (theme, _) = expect_context::<(ReadSignal<Theme>, WriteSignal<Theme>)>();
    theme
}

pub fn use_set_theme() -> WriteSignal<Theme> {
    let (_, set_theme) = expect_context::<(ReadSignal<Theme>, WriteSignal<Theme>)>();
    set_theme
}

fn detect_initial_theme() -> Theme {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            if let Ok(Some(saved)) = storage.get_item("theme") {
                return match saved.as_str() {
                    "dark" => Theme::Dark,
                    "light" => Theme::Light,
                    _ => detect_os_theme(),
                };
            }
        }
    }
    detect_os_theme()
}

fn detect_os_theme() -> Theme {
    let result = js_sys::eval("window.matchMedia('(prefers-color-scheme: dark)').matches");
    match result {
        Ok(val) if val.as_bool() == Some(true) => Theme::Dark,
        _ => Theme::Light,
    }
}

fn apply_theme(theme: Theme) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.document_element() {
            let _ = el.set_attribute("data-theme", theme.as_str());
        }
    }
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("theme", theme.as_str());
        }
    }
}

#[component]
pub fn ThemeProvider(children: Children) -> impl IntoView {
    let initial = detect_initial_theme();
    apply_theme(initial);
    let (theme, set_theme) = signal(initial);
    provide_context((theme, set_theme));

    Effect::new(move || {
        let t = theme.get();
        apply_theme(t);
    });

    children()
}
