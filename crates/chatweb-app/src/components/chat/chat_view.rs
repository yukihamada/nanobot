use leptos::prelude::*;

use crate::context::auth::use_auth;
use crate::types::chat::*;
use super::chat_bubble::ChatBubble;
use super::chat_input::ChatInput;

#[component]
pub fn ChatView() -> impl IntoView {
    let messages = RwSignal::new(Vec::<ChatMessage>::new());
    let streaming_text = RwSignal::new(String::new());
    let tool_steps = RwSignal::new(Vec::<ToolStep>::new());
    let loading = RwSignal::new(false);
    let thinking_info = RwSignal::new(Option::<(u32, u32)>::None);
    let show_login_modal = RwSignal::new(false);
    let auth = use_auth();

    let on_send = Callback::new(move |text: String| {
        if text.trim().is_empty() || loading.get_untracked() {
            return;
        }

        // Require login
        if !auth.authenticated.get_untracked() {
            show_login_modal.set(true);
            return;
        }

        // Add user message
        let user_msg = ChatMessage {
            role: MessageRole::User,
            content: text.clone(),
            model: None,
            tools_used: vec![],
            credits_used: None,
            timestamp: js_sys::Date::now(),
        };
        messages.update(|msgs| msgs.push(user_msg));
        loading.set(true);
        streaming_text.set(String::new());
        tool_steps.set(vec![]);
        thinking_info.set(None);

        let session_key = get_or_create_session_id();

        // SSE streaming
        wasm_bindgen_futures::spawn_local(async move {
            let req = ChatRequest {
                message: text,
                session_key: Some(session_key),
                model: None,
                tier: None,
            };

            let mut final_model: Option<String> = None;
            let mut final_credits: Option<f64> = None;
            let _collected_tools: Vec<ToolStep> = vec![];

            let result = crate::api::chat::stream_chat(&req, move |evt| {
                match evt {
                    SseEvent::Start { .. } => {}
                    SseEvent::ToolStart { tool, iteration, max_iter, args_preview } => {
                        thinking_info.set(Some((
                            iteration.unwrap_or(1),
                            max_iter.unwrap_or(1),
                        )));
                        tool_steps.update(|steps| {
                            steps.push(ToolStep {
                                tool,
                                status: ToolStatus::Running,
                                args_preview,
                                result: None,
                                duration_ms: None,
                                is_error: false,
                                iteration: iteration.unwrap_or(1),
                            });
                        });
                    }
                    SseEvent::ToolResult { tool, result, duration_ms, is_error, .. } => {
                        tool_steps.update(|steps| {
                            if let Some(step) = steps.iter_mut().rev().find(|s| s.tool == tool && s.status == ToolStatus::Running) {
                                step.status = if is_error.unwrap_or(false) { ToolStatus::Error } else { ToolStatus::Done };
                                step.result = result;
                                step.duration_ms = duration_ms;
                                step.is_error = is_error.unwrap_or(false);
                            }
                        });
                    }
                    SseEvent::Thinking { iteration, max_iter, .. } => {
                        thinking_info.set(Some((
                            iteration.unwrap_or(1),
                            max_iter.unwrap_or(1),
                        )));
                    }
                    SseEvent::ContentChunk { text } => {
                        streaming_text.update(|s| s.push_str(&text));
                    }
                    SseEvent::Content { content, model_used, credits_remaining, total_credits_used, .. } => {
                        if let Some(c) = content {
                            if !c.is_empty() {
                                streaming_text.set(c);
                            }
                        }
                        if let Some(m) = model_used {
                            streaming_text.update(|_| {});
                            let _ = js_sys::eval(&format!(
                                "window.__last_model='{}';window.__last_credits={};window.__last_used={}",
                                m,
                                credits_remaining.unwrap_or(0.0),
                                total_credits_used.unwrap_or(0.0),
                            ));
                        }
                    }
                    SseEvent::Error { content, .. } => {
                        let err = content.unwrap_or_else(|| "Unknown error".to_string());
                        streaming_text.set(err);
                    }
                    SseEvent::Done {} => {}
                }
            })
            .await;

            // Read back model/credits from JS globals
            if let Ok(v) = js_sys::eval("window.__last_model||''") {
                if let Some(m) = v.as_string() {
                    if !m.is_empty() {
                        final_model = Some(m);
                    }
                }
            }
            if let Ok(v) = js_sys::eval("window.__last_credits||0") {
                if let Some(c) = v.as_f64() {
                    auth.credits_remaining.set(c as i64);
                }
            }
            if let Ok(v) = js_sys::eval("window.__last_used||0") {
                final_credits = v.as_f64();
            }

            // Finalize
            let text = streaming_text.get_untracked();
            let tools = tool_steps.get_untracked();

            {
                let content = if !text.is_empty() {
                    text
                } else if result.is_err() {
                    format!("Error: {}", result.err().unwrap_or_default())
                } else {
                    "\u{ff08}\u{5fdc}\u{7b54}\u{3092}\u{53d6}\u{5f97}\u{3067}\u{304d}\u{307e}\u{305b}\u{3093}\u{3067}\u{3057}\u{305f}\u{3002}\u{3082}\u{3046}\u{4e00}\u{5ea6}\u{304a}\u{8a66}\u{3057}\u{304f}\u{3060}\u{3055}\u{3044}\u{ff09}".to_string()
                };

                let ai_msg = ChatMessage {
                    role: MessageRole::Assistant,
                    content,
                    model: final_model,
                    tools_used: tools,
                    credits_used: final_credits,
                    timestamp: js_sys::Date::now(),
                };
                messages.update(|msgs| msgs.push(ai_msg));
            }

            streaming_text.set(String::new());
            tool_steps.set(vec![]);
            thinking_info.set(None);
            loading.set(false);
        });
    });

    view! {
        <div class="flex flex-col h-full">
            // Message list
            <div class="flex-1 overflow-y-auto px-4 py-6 space-y-4" id="chat-messages">
                <Show
                    when=move || !messages.get().is_empty() || loading.get()
                    fallback=move || view! { <WelcomeScreen on_send=on_send /> }
                >
                    <div class="max-w-3xl mx-auto space-y-4">
                        <For
                            each=move || messages.get()
                            key=|msg| (msg.timestamp * 1000.0) as u64
                            let:msg
                        >
                            <ChatBubble message=msg />
                        </For>

                        // Tool progress (while streaming)
                        <Show when=move || !tool_steps.get().is_empty() && loading.get()>
                            <div class="bg-[var(--surface)] rounded-xl p-3 space-y-2 border border-[var(--border)]">
                                {move || thinking_info.get().map(|(iter, max)| {
                                    view! {
                                        <div class="text-xs text-[var(--muted)] mb-2">
                                            {format!("\u{30b9}\u{30c6}\u{30c3}\u{30d7} {}/{}", iter, max)}
                                        </div>
                                    }
                                })}
                                <For
                                    each=move || tool_steps.get()
                                    key=|s| format!("{}-{}", s.tool, s.iteration)
                                    let:step
                                >
                                    <ToolProgressItem step=step />
                                </For>
                            </div>
                        </Show>

                        // Streaming text
                        <Show when=move || { let t = streaming_text.get(); !t.is_empty() && loading.get() }>
                            <div class="flex justify-start">
                                <div class="max-w-[80%] rounded-2xl px-4 py-3 whitespace-pre-wrap break-words"
                                     style="background-color: var(--surface); color: var(--text);">
                                    <p>{move || streaming_text.get()}</p>
                                </div>
                            </div>
                        </Show>

                        // Loading dots
                        <Show when=move || loading.get() && streaming_text.get().is_empty() && tool_steps.get().is_empty()>
                            <div class="flex items-center gap-2 px-4 py-3">
                                <div class="flex gap-1">
                                    <span class="w-2 h-2 rounded-full animate-bounce" style="background-color: var(--brand-accent); animation-delay: 0ms"></span>
                                    <span class="w-2 h-2 rounded-full animate-bounce" style="background-color: var(--brand-accent); animation-delay: 150ms"></span>
                                    <span class="w-2 h-2 rounded-full animate-bounce" style="background-color: var(--brand-accent); animation-delay: 300ms"></span>
                                </div>
                            </div>
                        </Show>
                    </div>
                </Show>
            </div>

            // Input area
            <ChatInput on_send=on_send loading=loading />

            // Login modal
            <Show when=move || show_login_modal.get()>
                <LoginModal on_close=Callback::new(move |_: ()| show_login_modal.set(false)) />
            </Show>
        </div>
    }
}

/// Welcome screen shown when no messages exist.
/// Doubles as the landing page — shows real capabilities with clickable examples.
#[component]
fn WelcomeScreen(
    #[prop(into)] on_send: Callback<String>,
) -> impl IntoView {
    let send_example = move |text: &'static str| {
        let cb = on_send;
        move |_: leptos::ev::MouseEvent| {
            cb.run(text.to_string());
        }
    };

    view! {
        <div class="flex items-center justify-center h-full">
            <div class="max-w-2xl w-full px-4">
                // Brand
                <div class="text-center mb-8">
                    <div class="inline-block mb-3">
                        <div class="w-12 h-12 rounded-2xl flex items-center justify-center text-white text-lg font-bold mx-auto"
                             style="background: linear-gradient(135deg, var(--brand-accent), #a78bfa);">
                            "cw"
                        </div>
                    </div>
                    <h1 class="text-2xl sm:text-3xl font-bold tracking-tight mb-2">
                        <span style="background: linear-gradient(135deg, var(--brand-accent), #a78bfa); -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;">
                            "chatweb.ai"
                        </span>
                    </h1>
                    <p class="text-[var(--muted)] text-sm">
                        "\u{8a71}\u{3059}\u{3060}\u{3051}\u{3067}\u{3001}\u{691c}\u{7d22}\u{3082}\u{30b3}\u{30fc}\u{30c9}\u{5b9f}\u{884c}\u{3082}\u{753b}\u{50cf}\u{751f}\u{6210}\u{3082}\u{3002}"
                    </p>
                </div>

                // Clickable example prompts — grouped by what actually works
                <div class="space-y-3 mb-8">
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
                        <button
                            class="group text-left px-4 py-3 rounded-xl border border-[var(--border)] bg-[var(--surface)] hover:border-[var(--brand-accent)] transition-all"
                            on:click=send_example("\u{6771}\u{4eac}\u{306e}\u{4eca}\u{306e}\u{5929}\u{6c17}\u{3092}\u{8abf}\u{3079}\u{3066}")
                        >
                            <div class="flex items-center gap-3">
                                <span class="text-lg opacity-70 group-hover:opacity-100 transition-opacity">"~"</span>
                                <div>
                                    <div class="text-sm font-medium">"\u{5929}\u{6c17}\u{3092}\u{8abf}\u{3079}\u{308b}"</div>
                                    <div class="text-xs text-[var(--muted)]">"Open-Meteo API\u{3067}\u{30ea}\u{30a2}\u{30eb}\u{30bf}\u{30a4}\u{30e0}\u{53d6}\u{5f97}"</div>
                                </div>
                            </div>
                        </button>
                        <button
                            class="group text-left px-4 py-3 rounded-xl border border-[var(--border)] bg-[var(--surface)] hover:border-[var(--brand-accent)] transition-all"
                            on:click=send_example("\u{6700}\u{65b0}\u{306e}AI\u{30cb}\u{30e5}\u{30fc}\u{30b9}\u{3092}\u{691c}\u{7d22}\u{3057}\u{3066}\u{307e}\u{3068}\u{3081}\u{3066}")
                        >
                            <div class="flex items-center gap-3">
                                <span class="text-lg opacity-70 group-hover:opacity-100 transition-opacity">"/"</span>
                                <div>
                                    <div class="text-sm font-medium">"Web\u{691c}\u{7d22} + \u{8981}\u{7d04}"</div>
                                    <div class="text-xs text-[var(--muted)]">"Brave/Bing\u{3067}\u{691c}\u{7d22}\u{3057}\u{3066}AI\u{304c}\u{8981}\u{7d04}"</div>
                                </div>
                            </div>
                        </button>
                        <button
                            class="group text-left px-4 py-3 rounded-xl border border-[var(--border)] bg-[var(--surface)] hover:border-[var(--brand-accent)] transition-all"
                            on:click=send_example("\u{5bcc}\u{58eb}\u{5c71}\u{3068}\u{685c}\u{306e}\u{7f8e}\u{3057}\u{3044}\u{753b}\u{50cf}\u{3092}\u{751f}\u{6210}\u{3057}\u{3066}")
                        >
                            <div class="flex items-center gap-3">
                                <span class="text-lg opacity-70 group-hover:opacity-100 transition-opacity">"*"</span>
                                <div>
                                    <div class="text-sm font-medium">"\u{753b}\u{50cf}\u{751f}\u{6210}"</div>
                                    <div class="text-xs text-[var(--muted)]">"Flux\u{30e2}\u{30c7}\u{30eb}\u{3067}\u{30c6}\u{30ad}\u{30b9}\u{30c8}\u{304b}\u{3089}\u{753b}\u{50cf}\u{3092}\u{4f5c}\u{6210}"</div>
                                </div>
                            </div>
                        </button>
                        <button
                            class="group text-left px-4 py-3 rounded-xl border border-[var(--border)] bg-[var(--surface)] hover:border-[var(--brand-accent)] transition-all"
                            on:click=send_example("echo 'Hello' | base64 \u{3092}\u{5b9f}\u{884c}\u{3057}\u{3066}")
                        >
                            <div class="flex items-center gap-3">
                                <span class="text-lg opacity-70 group-hover:opacity-100 transition-opacity">">"</span>
                                <div>
                                    <div class="text-sm font-medium">"\u{30b3}\u{30fc}\u{30c9}\u{5b9f}\u{884c}"</div>
                                    <div class="text-xs text-[var(--muted)]">"\u{30b5}\u{30f3}\u{30c9}\u{30dc}\u{30c3}\u{30af}\u{30b9}\u{3067}\u{30b7}\u{30a7}\u{30eb}\u{3092}\u{5b9f}\u{884c}"</div>
                                </div>
                            </div>
                        </button>
                    </div>
                </div>

                // Honest capability badges
                <div class="flex flex-wrap justify-center gap-2 mb-4">
                    <CapBadge label="\u{691c}\u{7d22}" detail="Brave + Bing" />
                    <CapBadge label="\u{30b3}\u{30fc}\u{30c9}" detail="Shell sandbox" />
                    <CapBadge label="\u{753b}\u{50cf}" detail="Flux AI" />
                    <CapBadge label="\u{5929}\u{6c17}" detail="Open-Meteo" />
                    <CapBadge label="Wikipedia" detail="" />
                    <CapBadge label="\u{8a08}\u{7b97}" detail="" />
                    <CapBadge label="QR" detail="" />
                    <CapBadge label="LINE" detail="" />
                </div>
                <p class="text-center text-xs text-[var(--muted)]">
                    "Claude \u{00b7} GPT-4o \u{00b7} Gemini \u{00b7} Nemotron \u{304b}\u{3089}\u{81ea}\u{52d5}\u{9078}\u{629e} \u{00b7} \u{30ea}\u{30a2}\u{30eb}\u{30bf}\u{30a4}\u{30e0}\u{30b9}\u{30c8}\u{30ea}\u{30fc}\u{30df}\u{30f3}\u{30b0}"
                </p>
            </div>
        </div>
    }
}

#[component]
fn CapBadge(
    label: &'static str,
    detail: &'static str,
) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-1 px-2.5 py-1 rounded-full text-xs border border-[var(--border)] bg-[var(--surface)] text-[var(--muted)]">
            <span class="font-medium text-[var(--text)]">{label}</span>
            {(!detail.is_empty()).then(|| view! {
                <span class="opacity-60">{detail}</span>
            })}
        </span>
    }
}

#[component]
fn LoginModal(
    #[prop(into)] on_close: Callback<()>,
) -> impl IntoView {
    let auth = use_auth();
    let email_input = RwSignal::new(String::new());
    let code_input = RwSignal::new(String::new());
    let step = RwSignal::new(0u8); // 0=initial, 1=code entry
    let error_msg = RwSignal::new(Option::<String>::None);
    let sending = RwSignal::new(false);
    let pending_email = RwSignal::new(String::new());

    let handle_google = move |_: leptos::ev::MouseEvent| {
        let _ = web_sys::window().and_then(|w| {
            w.location().set_href("/auth/google").ok()
        });
    };

    let auth_for_email = auth.clone();
    let handle_email_submit = Callback::new(move |_: ()| {
        let email = email_input.get_untracked().trim().to_string();
        if email.is_empty() || !email.contains('@') {
            error_msg.set(Some("\u{30e1}\u{30fc}\u{30eb}\u{30a2}\u{30c9}\u{30ec}\u{30b9}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}".to_string()));
            return;
        }
        error_msg.set(None);
        sending.set(true);
        pending_email.set(email.clone());

        let auth = auth_for_email.clone();
        let on_close = on_close;
        wasm_bindgen_futures::spawn_local(async move {
            match crate::api::auth::auth_email(&email).await {
                Ok(res) => {
                    if res.pending_verification.unwrap_or(false) {
                        step.set(1);
                    } else if res.token.is_some() {
                        if let Ok(me) = crate::api::auth::fetch_me().await {
                            auth.update_from(&me);
                        }
                        on_close.run(());
                    } else if let Some(err) = res.error {
                        error_msg.set(Some(err));
                    }
                }
                Err(e) => {
                    error_msg.set(Some(e));
                }
            }
            sending.set(false);
        });
    });

    let handle_verify = Callback::new(move |_: ()| {
        let code = code_input.get_untracked().trim().to_string();
        let email = pending_email.get_untracked();
        if code.len() != 6 {
            error_msg.set(Some("6\u{6841}\u{306e}\u{30b3}\u{30fc}\u{30c9}\u{3092}\u{5165}\u{529b}\u{3057}\u{3066}\u{304f}\u{3060}\u{3055}\u{3044}".to_string()));
            return;
        }
        error_msg.set(None);
        sending.set(true);

        let auth = auth.clone();
        let on_close = on_close;
        wasm_bindgen_futures::spawn_local(async move {
            match crate::api::auth::verify_code(&email, &code).await {
                Ok(res) => {
                    if res.token.is_some() {
                        if let Ok(me) = crate::api::auth::fetch_me().await {
                            auth.update_from(&me);
                        }
                        on_close.run(());
                    } else if let Some(err) = res.error {
                        error_msg.set(Some(err));
                    } else {
                        error_msg.set(Some("\u{8a8d}\u{8a3c}\u{306b}\u{5931}\u{6557}\u{3057}\u{307e}\u{3057}\u{305f}".to_string()));
                    }
                }
                Err(e) => {
                    error_msg.set(Some(e));
                }
            }
            sending.set(false);
        });
    });

    let handle_backdrop = move |_: leptos::ev::MouseEvent| {
        on_close.run(());
    };

    view! {
        <div
            class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
            on:click=handle_backdrop
        >
            <div
                class="bg-[var(--bg)] rounded-2xl p-6 max-w-sm w-full mx-4 border border-[var(--border)] shadow-2xl"
                on:click=move |e: leptos::ev::MouseEvent| e.stop_propagation()
            >
                <div class="text-center mb-6">
                    <div class="w-10 h-10 rounded-xl flex items-center justify-center text-white text-sm font-bold mx-auto mb-3"
                         style="background: linear-gradient(135deg, var(--brand-accent), #a78bfa);">
                        "cw"
                    </div>
                    <h2 class="text-lg font-bold mb-1">"\u{30ed}\u{30b0}\u{30a4}\u{30f3}\u{3057}\u{3066}\u{59cb}\u{3081}\u{3088}\u{3046}"</h2>
                    <p class="text-sm text-[var(--muted)]">
                        "\u{30a2}\u{30ab}\u{30a6}\u{30f3}\u{30c8}\u{3092}\u{4f5c}\u{6210}\u{3057}\u{3066}\u{3001}AI\u{3068}\u{4f1a}\u{8a71}\u{3092}\u{59cb}\u{3081}\u{307e}\u{3057}\u{3087}\u{3046}"
                    </p>
                </div>

                // Error message
                {move || error_msg.get().map(|msg| view! {
                    <div class="mb-4 px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/20 text-red-500 text-sm text-center">
                        {msg}
                    </div>
                })}

                <Show
                    when=move || step.get() == 0
                    fallback=move || view! {
                        // Step 1: Enter verification code
                        <div class="space-y-3">
                            <p class="text-sm text-center text-[var(--muted)] mb-2">
                                {move || format!("{} \u{306b}\u{8a8d}\u{8a3c}\u{30b3}\u{30fc}\u{30c9}\u{3092}\u{9001}\u{4fe1}\u{3057}\u{307e}\u{3057}\u{305f}", pending_email.get())}
                            </p>
                            <input
                                type="text"
                                inputmode="numeric"
                                maxlength="6"
                                placeholder="6\u{6841}\u{306e}\u{30b3}\u{30fc}\u{30c9}"
                                class="w-full px-4 py-3 rounded-xl border border-[var(--border)] bg-[var(--surface)] text-[var(--text)] text-center text-lg tracking-[0.5em] focus:outline-none focus:ring-2 focus:ring-[var(--brand-accent)]"
                                prop:value=move || code_input.get()
                                on:input=move |e| code_input.set(leptos::prelude::event_target_value(&e))
                            />
                            <button
                                class="w-full px-4 py-3 rounded-xl text-white font-medium text-sm transition-colors disabled:opacity-50"
                                style="background-color: var(--brand-accent);"
                                on:click=move |_| handle_verify.run(())
                                prop:disabled=move || sending.get()
                            >
                                {move || if sending.get() { "\u{78ba}\u{8a8d}\u{4e2d}..." } else { "\u{78ba}\u{8a8d}" }}
                            </button>
                            <button
                                class="w-full text-sm text-[var(--muted)] hover:text-[var(--text)] transition-colors"
                                on:click=move |_| { step.set(0); error_msg.set(None); }
                            >
                                "\u{623b}\u{308b}"
                            </button>
                        </div>
                    }
                >
                    // Step 0: Choose method
                    <div class="space-y-3">
                        // Google login
                        <button
                            class="w-full flex items-center justify-center gap-3 px-4 py-3 rounded-xl border border-[var(--border)] bg-[var(--surface)] hover:bg-[var(--surface2)] transition-colors font-medium text-sm"
                            on:click=handle_google
                        >
                            <svg width="18" height="18" viewBox="0 0 18 18" xmlns="http://www.w3.org/2000/svg">
                                <path d="M17.64 9.2c0-.637-.057-1.251-.164-1.84H9v3.481h4.844a4.14 4.14 0 01-1.796 2.716v2.259h2.908c1.702-1.567 2.684-3.875 2.684-6.615z" fill="#4285F4"/>
                                <path d="M9 18c2.43 0 4.467-.806 5.956-2.18l-2.908-2.259c-.806.54-1.837.86-3.048.86-2.344 0-4.328-1.584-5.036-3.711H.957v2.332A8.997 8.997 0 009 18z" fill="#34A853"/>
                                <path d="M3.964 10.71A5.41 5.41 0 013.682 9c0-.593.102-1.17.282-1.71V4.958H.957A8.996 8.996 0 000 9c0 1.452.348 2.827.957 4.042l3.007-2.332z" fill="#FBBC05"/>
                                <path d="M9 3.58c1.321 0 2.508.454 3.44 1.345l2.582-2.58C13.463.891 11.426 0 9 0A8.997 8.997 0 00.957 4.958L3.964 6.29C4.672 4.163 6.656 2.58 9 3.58z" fill="#EA4335"/>
                            </svg>
                            "Google\u{3067}\u{30ed}\u{30b0}\u{30a4}\u{30f3}"
                        </button>

                        <div class="flex items-center gap-3 my-2">
                            <div class="flex-1 h-px bg-[var(--border)]"></div>
                            <span class="text-xs text-[var(--muted)]">"or"</span>
                            <div class="flex-1 h-px bg-[var(--border)]"></div>
                        </div>

                        // Email input
                        <input
                            type="email"
                            placeholder="\u{30e1}\u{30fc}\u{30eb}\u{30a2}\u{30c9}\u{30ec}\u{30b9}"
                            class="w-full px-4 py-3 rounded-xl border border-[var(--border)] bg-[var(--surface)] text-[var(--text)] placeholder-[var(--muted)] focus:outline-none focus:ring-2 focus:ring-[var(--brand-accent)] text-sm"
                            prop:value=move || email_input.get()
                            on:input=move |e| email_input.set(leptos::prelude::event_target_value(&e))
                            on:keydown=move |e: leptos::ev::KeyboardEvent| {
                                if e.key() == "Enter" {
                                    e.prevent_default();
                                    // Trigger submit via click simulation
                                    handle_email_submit.run(());
                                }
                            }
                        />
                        <button
                            class="w-full px-4 py-3 rounded-xl text-white font-medium text-sm transition-colors disabled:opacity-50"
                            style="background-color: var(--brand-accent);"
                            on:click=move |_| handle_email_submit.run(())
                            prop:disabled=move || sending.get()
                        >
                            {move || if sending.get() { "\u{9001}\u{4fe1}\u{4e2d}..." } else { "\u{30e1}\u{30fc}\u{30eb}\u{3067}\u{7d9a}\u{3051}\u{308b}" }}
                        </button>
                    </div>
                </Show>

                <button
                    class="w-full mt-4 text-sm text-[var(--muted)] hover:text-[var(--text)] transition-colors"
                    on:click=move |_| on_close.run(())
                >
                    "\u{9589}\u{3058}\u{308b}"
                </button>
            </div>
        </div>
    }
}

#[component]
fn ToolProgressItem(step: ToolStep) -> impl IntoView {
    let icon = tool_icon(&step.tool);
    let label = tool_label(&step.tool).to_string();
    let is_running = step.status == ToolStatus::Running;
    let _is_error = step.is_error;

    let status_icon = match step.status {
        ToolStatus::Running => "\u{23f3}",
        ToolStatus::Done => "\u{2705}",
        ToolStatus::Error => "\u{274c}",
    };

    let duration_str = step.duration_ms.map(|ms| {
        if ms < 1000 { format!("{}ms", ms) } else { format!("{:.1}s", ms as f64 / 1000.0) }
    });

    view! {
        <div class="flex items-center gap-2 text-sm" class:animate-pulse=is_running>
            <span>{status_icon}</span>
            <span>{icon}</span>
            <span class="font-medium">{label}</span>
            {step.args_preview.as_ref().map(|p| view! {
                <span class="text-[var(--muted)] text-xs truncate max-w-[200px]">{p.clone()}</span>
            })}
            {duration_str.map(|d| view! {
                <span class="text-[var(--muted)] text-xs ml-auto">{d}</span>
            })}
        </div>
    }
}

fn get_or_create_session_id() -> String {
    let storage = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten();

    if let Some(ref s) = storage {
        if let Ok(Some(id)) = s.get_item("session_id") {
            return id;
        }
    }

    let id = format!(
        "wasm-{}",
        js_sys::Math::random().to_string().replace("0.", "")
    );
    if let Some(s) = storage {
        let _ = s.set_item("session_id", &id);
    }
    id
}
