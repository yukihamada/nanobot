use leptos::prelude::*;

use crate::context::auth::use_auth;
use crate::context::brand::use_brand;
use crate::context::theme::{use_theme, use_set_theme, Theme};

#[component]
pub fn Header(
    #[prop(into)] on_toggle_sidebar: Callback<()>,
) -> impl IntoView {
    let brand = use_brand();
    let theme = use_theme();
    let set_theme = use_set_theme();
    let auth = use_auth();

    let toggle_theme = move |_| {
        let current = theme.get();
        set_theme.set(match current {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Light,
        });
    };

    view! {
        <header class="h-14 border-b border-[var(--border)] bg-[var(--surface)] flex items-center px-4 shrink-0 gap-2">
            // Mobile sidebar toggle
            <button
                class="w-9 h-9 rounded-lg flex items-center justify-center hover:bg-[var(--surface2)] transition-colors md:hidden"
                on:click=move |_| on_toggle_sidebar.run(())
            >
                <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/></svg>
            </button>

            <h1 class="text-lg font-bold" style="color: var(--brand-accent)">
                {brand.name()}
            </h1>

            <div class="flex-1"></div>

            // Credits display
            <Show when=move || auth.authenticated.get()>
                <div class="text-xs text-[var(--muted)] hidden sm:block">
                    {move || format!("{} credits", auth.credits_remaining.get())}
                </div>
            </Show>

            // Theme toggle
            <button
                class="w-9 h-9 rounded-lg flex items-center justify-center hover:bg-[var(--surface2)] transition-colors"
                on:click=toggle_theme
                title="Toggle theme"
            >
                {move || if theme.get() == Theme::Dark { "☀️" } else { "🌙" }}
            </button>
        </header>
    }
}
