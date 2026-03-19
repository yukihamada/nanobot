use leptos::prelude::*;

use crate::context::auth::use_auth;

#[component]
pub fn Sidebar(
    show: RwSignal<bool>,
) -> impl IntoView {
    let auth = use_auth();

    let new_chat = move |_| {
        // Clear session to start fresh
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                let _ = storage.remove_item("session_id");
            }
            // Reload to reset state
            let _ = window.location().reload();
        }
    };

    view! {
        // Overlay for mobile
        <Show when=move || show.get()>
            <div
                class="fixed inset-0 bg-black/50 z-40 md:hidden"
                on:click=move |_| show.set(false)
            ></div>
        </Show>

        <aside
            class="w-64 border-r border-[var(--border)] bg-[var(--surface)] flex-col z-50 shrink-0"
            class=("fixed", move || show.get())
            class=("inset-y-0", move || show.get())
            class=("left-0", move || show.get())
            class=("hidden", move || !show.get())
            class=("md:flex", true)
        >
            <div class="p-4">
                <button
                    class="w-full py-2.5 px-4 rounded-xl text-white text-sm font-medium transition-colors hover:opacity-90"
                    style="background-color: var(--brand-accent)"
                    on:click=new_chat
                >
                    "+ New Chat"
                </button>
            </div>

            <div class="flex-1 overflow-y-auto px-2">
                <p class="text-xs text-[var(--muted)] px-2 py-4">"会話履歴は今後追加予定"</p>
            </div>

            // User info at bottom
            <div class="p-3 border-t border-[var(--border)]">
                <Show
                    when=move || auth.authenticated.get()
                    fallback=move || view! {
                        <button
                            class="w-full py-2 px-3 rounded-lg text-sm text-[var(--text)] hover:bg-[var(--surface2)] transition-colors text-left"
                            on:click=move |_| {
                                // Navigate to login (TODO: modal)
                                if let Some(w) = web_sys::window() {
                                    let _ = w.location().set_href("/auth/google");
                                }
                            }
                        >
                            "🔑 ログイン"
                        </button>
                    }
                >
                    <div class="text-sm">
                        <div class="font-medium text-[var(--text)] truncate">
                            {move || auth.display_name.get().unwrap_or_else(|| "User".to_string())}
                        </div>
                        <div class="text-xs text-[var(--muted)]">
                            {move || format!("{} credits", auth.credits_remaining.get())}
                        </div>
                    </div>
                </Show>
            </div>
        </aside>
    }
}
