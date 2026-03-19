use leptos::prelude::*;

use crate::types::chat::*;

#[component]
pub fn ChatBubble(message: ChatMessage) -> impl IntoView {
    let is_user = message.role == MessageRole::User;
    let content = message.content.clone();
    let model_label = message.model.clone();
    let model_label2 = model_label.clone();
    let has_tools = !message.tools_used.is_empty();
    let tools = message.tools_used.clone();
    let credits = message.credits_used;

    view! {
        <div class=if is_user { "flex justify-end" } else { "flex justify-start" }>
            <div
                class=if is_user {
                    "max-w-[80%] rounded-2xl px-4 py-3 whitespace-pre-wrap break-words text-white"
                } else {
                    "max-w-[80%] rounded-2xl px-4 py-3 whitespace-pre-wrap break-words"
                }
                style=if is_user {
                    "background-color: var(--brand-accent);"
                } else {
                    "background-color: var(--surface); color: var(--text);"
                }
            >
                // Tool summary (collapsed)
                <Show when=move || has_tools>
                    <div class="mb-2 space-y-1">
                        {tools.iter().map(|step| {
                            let icon = tool_icon(&step.tool);
                            let label = tool_label(&step.tool).to_string();
                            let status = if step.is_error { "❌" } else { "✅" };
                            let dur = step.duration_ms.map(|ms| {
                                if ms < 1000 { format!("{}ms", ms) } else { format!("{:.1}s", ms as f64 / 1000.0) }
                            });
                            view! {
                                <div class="flex items-center gap-1 text-xs opacity-70">
                                    <span>{status}</span>
                                    <span>{icon}</span>
                                    <span>{label}</span>
                                    {dur.map(|d| view! { <span class="ml-1">{d}</span> })}
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </Show>

                <p>{content}</p>

                // Model + credits footer
                <Show when=move || model_label.is_some() || credits.is_some()>
                    <div class="mt-1 text-xs opacity-50 flex gap-2">
                        {model_label2.clone().map(|m| view! { <span>{m}</span> })}
                        {credits.map(|c| view! { <span>{format!("{:.0} credits", c)}</span> })}
                    </div>
                </Show>
            </div>
        </div>
    }
}
