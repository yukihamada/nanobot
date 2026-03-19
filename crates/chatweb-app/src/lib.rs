pub mod api;
pub mod components;
pub mod context;
pub mod pages;
pub mod types;

use leptos::prelude::*;

use context::auth::AuthProvider;
use context::brand::BrandProvider;
use context::theme::ThemeProvider;
use pages::home::HomePage;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <BrandProvider>
            <ThemeProvider>
                <AuthProvider>
                    <main class="h-screen bg-[var(--bg)] text-[var(--text)]">
                        <HomePage />
                    </main>
                </AuthProvider>
            </ThemeProvider>
        </BrandProvider>
    }
}
