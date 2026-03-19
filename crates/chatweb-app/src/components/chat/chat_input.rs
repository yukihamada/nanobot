use leptos::prelude::*;
use leptos::ev;
use wasm_bindgen::JsCast;

#[component]
pub fn ChatInput(
    #[prop(into)] on_send: Callback<String>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    let text = RwSignal::new(String::new());

    let do_send = move || {
        let val = text.get_untracked();
        if !val.trim().is_empty() && !loading.get_untracked() {
            on_send.run(val);
            text.set(String::new());
        }
    };

    let handle_keydown = move |e: ev::KeyboardEvent| {
        if e.key() == "Enter" && !e.shift_key() {
            e.prevent_default();
            do_send();
        }
    };

    view! {
        <div class="border-t border-[var(--border)] px-4 py-3 bg-[var(--surface)]">
            <div class="flex items-end gap-2 max-w-3xl mx-auto">
                <textarea
                    class="flex-1 resize-none rounded-xl px-4 py-3 bg-[var(--bg)] border border-[var(--border)] text-[var(--text)] placeholder-[var(--muted)] focus:outline-none focus:ring-2 focus:ring-[var(--brand-accent)] min-h-[44px] max-h-[200px]"
                    placeholder="メッセージを入力..."
                    rows=1
                    prop:value=move || text.get()
                    on:input=move |e| {
                        let val = event_target_value(&e);
                        text.set(val);
                    }
                    on:keydown=handle_keydown
                    prop:disabled=move || loading.get()
                />
                <button
                    class="shrink-0 w-11 h-11 rounded-xl flex items-center justify-center text-white transition-colors disabled:opacity-50"
                    style="background-color: var(--brand-accent)"
                    on:click=move |_| do_send()
                    prop:disabled=move || loading.get() || text.get().trim().is_empty()
                >
                    <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <line x1="22" y1="2" x2="11" y2="13"></line>
                        <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
                    </svg>
                </button>
            </div>
        </div>
    }
}

fn event_target_value(e: &ev::Event) -> String {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        .map(|el| el.value())
        .unwrap_or_default()
}
