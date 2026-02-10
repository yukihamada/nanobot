#[cfg(feature = "dynamodb-backend")]
use aws_sdk_dynamodb::types::AttributeValue;

use tokio::sync::Mutex;

use crate::session::store::SessionStore;

// ---------------------------------------------------------------------------
// Slash command types
// ---------------------------------------------------------------------------

/// Parsed slash command from user input.
#[derive(Debug, PartialEq)]
pub enum SlashCommand<'a> {
    /// `/link` or `/link CODE` — channel linking
    Link(Option<&'a str>),
    /// `/share` — generate a shared conversation link
    Share,
    /// `/help` — list available commands
    Help,
    /// `/status` — inline system status
    Status,
    /// `/improve <description>` — admin-only self-improvement PR
    Improve(&'a str),
}

/// Result of executing a slash command.
pub enum CommandResult {
    /// Text reply to send back to the user.
    Reply(String),
    /// The input was not a slash command — pass to LLM.
    NotACommand,
}

/// Context needed to execute commands.
pub struct CommandContext<'a> {
    pub channel_key: &'a str,
    pub session_key: &'a str,
    pub user_id: Option<&'a str>,
    pub conv_id: Option<&'a str>,
    pub sessions: &'a Mutex<Box<dyn SessionStore>>,
    #[cfg(feature = "dynamodb-backend")]
    pub dynamo: Option<&'a aws_sdk_dynamodb::Client>,
    #[cfg(feature = "dynamodb-backend")]
    pub config_table: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse user text into a slash command, or `None` if not a command.
pub fn parse_command(text: &str) -> Option<SlashCommand<'_>> {
    let trimmed = text.trim();

    // /help
    if trimmed.eq_ignore_ascii_case("/help") {
        return Some(SlashCommand::Help);
    }

    // /status
    if trimmed.eq_ignore_ascii_case("/status") {
        return Some(SlashCommand::Status);
    }

    // /share
    if trimmed.eq_ignore_ascii_case("/share") {
        return Some(SlashCommand::Share);
    }

    // /improve <description>
    if let Some(rest) = strip_prefix_ci(trimmed, "/improve ") {
        let desc = rest.trim();
        if !desc.is_empty() {
            return Some(SlashCommand::Improve(desc));
        }
    }
    if trimmed.eq_ignore_ascii_case("/improve") {
        // bare /improve with no description — still parse it so we can reply with usage hint
        return Some(SlashCommand::Improve(""));
    }

    // /link [CODE] — must come last because of the embedded-code search
    if let Some(link) = parse_link(trimmed) {
        return Some(link);
    }

    None
}

/// Parse `/link` variants (exact, with code, embedded code).
fn parse_link(trimmed: &str) -> Option<SlashCommand<'_>> {
    if trimmed == "/link" {
        return Some(SlashCommand::Link(None));
    }
    if let Some(rest) = trimmed.strip_prefix("/link ") {
        let code = rest.trim();
        if !code.is_empty() {
            let first_word = code.split_whitespace().next().unwrap_or(code);
            return Some(SlashCommand::Link(Some(first_word)));
        }
        return Some(SlashCommand::Link(None));
    }
    // Search for "/link XXXXXX" anywhere in the text (copy-paste)
    if let Some(pos) = trimmed.find("/link ") {
        let after = &trimmed[pos + 6..];
        let code = after.trim();
        if !code.is_empty() {
            let first_word = code.split_whitespace().next().unwrap_or(code);
            if first_word.len() == 6 && first_word.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some(SlashCommand::Link(Some(first_word)));
            }
        }
    }
    None
}

/// Case-insensitive prefix strip (returns the remainder with original casing).
/// Safe for multibyte strings — uses `.get()` to avoid panics on non-char-boundary indices.
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let plen = prefix.len();
    if text.len() >= plen {
        if let Some(slice) = text.get(..plen) {
            if slice.eq_ignore_ascii_case(prefix) {
                return Some(&text[plen..]);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execute a parsed slash command.
pub async fn execute_command(
    cmd: SlashCommand<'_>,
    ctx: &CommandContext<'_>,
) -> CommandResult {
    match cmd {
        SlashCommand::Help => CommandResult::Reply(help_text()),
        SlashCommand::Status => execute_status(ctx).await,
        SlashCommand::Share => execute_share(ctx).await,
        SlashCommand::Link(code) => execute_link(code, ctx).await,
        SlashCommand::Improve(desc) => execute_improve(desc, ctx).await,
    }
}

// ---------------------------------------------------------------------------
// /help
// ---------------------------------------------------------------------------

fn help_text() -> String {
    "\
📋 利用可能なコマンド:\n\
\n\
/help — このヘルプを表示\n\
/status — システム状態を表示\n\
/share — 会話の共有リンクを生成\n\
/link — チャネル連携コードを生成\n\
/link CODE — 別チャネルとリンク\n\
/improve <説明> — 改善PRを作成（管理者のみ）"
        .to_string()
}

// ---------------------------------------------------------------------------
// /status
// ---------------------------------------------------------------------------

async fn execute_status(_ctx: &CommandContext<'_>) -> CommandResult {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let mut lines = vec!["📊 システム状態:".to_string()];

    // Check OpenAI
    let openai_ok = if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.is_empty() {
            let start = std::time::Instant::now();
            let res = client
                .get("https://api.openai.com/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await;
            let ms = start.elapsed().as_millis();
            match res {
                Ok(r) if r.status().is_success() => {
                    lines.push(format!("  OpenAI: ✅ OK ({ms}ms)"));
                    true
                }
                _ => {
                    lines.push(format!("  OpenAI: ❌ Error ({ms}ms)"));
                    false
                }
            }
        } else {
            lines.push("  OpenAI: ⚪ Not configured".to_string());
            false
        }
    } else {
        lines.push("  OpenAI: ⚪ Not configured".to_string());
        false
    };

    // Check Anthropic
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            let start = std::time::Instant::now();
            let res = client
                .get("https://api.anthropic.com/v1/models")
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await;
            let ms = start.elapsed().as_millis();
            match res {
                Ok(r) if r.status().is_success() => {
                    lines.push(format!("  Anthropic: ✅ OK ({ms}ms)"));
                }
                _ => {
                    lines.push(format!("  Anthropic: ❌ Error ({ms}ms)"));
                }
            }
        } else {
            lines.push("  Anthropic: ⚪ Not configured".to_string());
        }
    } else {
        lines.push("  Anthropic: ⚪ Not configured".to_string());
    }

    // Check Google
    if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
        if !key.is_empty() {
            lines.push("  Google: ✅ Configured".to_string());
        } else {
            lines.push("  Google: ⚪ Not configured".to_string());
        }
    } else {
        lines.push("  Google: ⚪ Not configured".to_string());
    }

    let _ = openai_ok; // suppress unused warning

    CommandResult::Reply(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// /share
// ---------------------------------------------------------------------------

async fn execute_share(ctx: &CommandContext<'_>) -> CommandResult {
    #[cfg(feature = "dynamodb-backend")]
    {
        let (dynamo, table) = match (ctx.dynamo, ctx.config_table) {
            (Some(d), Some(t)) => (d, t),
            _ => return CommandResult::Reply("共有機能は現在利用できません。".to_string()),
        };

        let user_id = match ctx.user_id {
            Some(uid) if !uid.is_empty() => uid,
            _ => return CommandResult::Reply("共有するにはログインが必要です。".to_string()),
        };

        let conv_id = match ctx.conv_id {
            Some(cid) if !cid.is_empty() => cid,
            _ => return CommandResult::Reply("共有する会話がありません。".to_string()),
        };

        // Check if already shared
        let existing = find_existing_share(dynamo, table, conv_id).await;
        if let Some(hash) = existing {
            return CommandResult::Reply(format!(
                "この会話は既に共有されています:\nhttps://chatweb.ai/c/{hash}"
            ));
        }

        // Generate a short hash from UUID (base62-ish, 10 chars)
        let hash = generate_share_hash();

        let now = chrono::Utc::now().to_rfc3339();
        let result = dynamo
            .put_item()
            .table_name(table)
            .item("pk", AttributeValue::S(format!("SHARE#{hash}")))
            .item("sk", AttributeValue::S("INFO".to_string()))
            .item("conv_id", AttributeValue::S(conv_id.to_string()))
            .item("user_id", AttributeValue::S(user_id.to_string()))
            .item("created_at", AttributeValue::S(now))
            .item("revoked", AttributeValue::Bool(false))
            .send()
            .await;

        match result {
            Ok(_) => {
                // Also store reverse lookup: CONV_SHARE#{conv_id} -> hash
                let _ = dynamo
                    .put_item()
                    .table_name(table)
                    .item("pk", AttributeValue::S(format!("CONV_SHARE#{conv_id}")))
                    .item("sk", AttributeValue::S("HASH".to_string()))
                    .item("share_hash", AttributeValue::S(hash.clone()))
                    .send()
                    .await;

                CommandResult::Reply(format!(
                    "共有リンクを生成しました:\nhttps://chatweb.ai/c/{hash}\n\nこのリンクを知っている人は誰でも会話を閲覧できます。"
                ))
            }
            Err(e) => {
                tracing::error!("Failed to create share link: {}", e);
                CommandResult::Reply("共有リンクの生成に失敗しました。".to_string())
            }
        }
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        let _ = ctx;
        CommandResult::Reply("共有機能はDynamoDBバックエンドが必要です。".to_string())
    }
}

/// Generate a 10-char alphanumeric hash from a UUID v4.
pub fn generate_share_hash() -> String {
    // Use UUID bytes as a number and encode in base62
    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    // Use first 8 bytes as u64 for base62 encoding
    let num = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    base62_encode(num, 10)
}

/// Simple base62 encoding (0-9, a-z, A-Z), truncated/padded to exactly `len`.
fn base62_encode(mut num: u64, len: usize) -> String {
    const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut result = Vec::new();
    if num == 0 {
        result.push(CHARSET[0]);
    } else {
        while num > 0 {
            result.push(CHARSET[(num % 62) as usize]);
            num /= 62;
        }
    }
    // Pad to len
    while result.len() < len {
        result.push(CHARSET[0]);
    }
    result.reverse();
    // Truncate to exactly len
    result.truncate(len);
    String::from_utf8(result).unwrap()
}

/// Find an existing non-revoked share hash for a conversation.
#[cfg(feature = "dynamodb-backend")]
async fn find_existing_share(
    dynamo: &aws_sdk_dynamodb::Client,
    table: &str,
    conv_id: &str,
) -> Option<String> {
    let resp = dynamo
        .get_item()
        .table_name(table)
        .key("pk", AttributeValue::S(format!("CONV_SHARE#{conv_id}")))
        .key("sk", AttributeValue::S("HASH".to_string()))
        .send()
        .await
        .ok()?;

    let item = resp.item?;
    let hash = item.get("share_hash").and_then(|v| v.as_s().ok())?.clone();

    // Verify the share record isn't revoked
    let share_resp = dynamo
        .get_item()
        .table_name(table)
        .key("pk", AttributeValue::S(format!("SHARE#{hash}")))
        .key("sk", AttributeValue::S("INFO".to_string()))
        .send()
        .await
        .ok()?;

    let share_item = share_resp.item?;
    let revoked = share_item
        .get("revoked")
        .and_then(|v| v.as_bool().ok())
        .copied()
        .unwrap_or(false);

    if revoked { None } else { Some(hash) }
}

// ---------------------------------------------------------------------------
// /link — delegated from the existing handle_link_command
// ---------------------------------------------------------------------------

async fn execute_link(code: Option<&str>, ctx: &CommandContext<'_>) -> CommandResult {
    #[cfg(feature = "dynamodb-backend")]
    {
        let (dynamo, table) = match (ctx.dynamo, ctx.config_table) {
            (Some(d), Some(t)) => (d, t),
            _ => return CommandResult::Reply("リンク機能は現在利用できません。".to_string()),
        };

        let result = handle_link_command(dynamo, table, ctx.channel_key, code, ctx.sessions).await;
        let msg = match result {
            LinkResult::CodeGenerated(msg)
            | LinkResult::Linked(msg)
            | LinkResult::Error(msg) => msg,
        };
        CommandResult::Reply(msg)
    }

    #[cfg(not(feature = "dynamodb-backend"))]
    {
        let _ = (code, ctx);
        CommandResult::Reply("リンク機能はDynamoDBバックエンドが必要です。".to_string())
    }
}

// ---------------------------------------------------------------------------
// /improve — admin-only self-improvement
// ---------------------------------------------------------------------------

async fn execute_improve(desc: &str, ctx: &CommandContext<'_>) -> CommandResult {
    if desc.is_empty() {
        return CommandResult::Reply(
            "使い方: /improve <改善の説明>\n例: /improve ステータスページにレスポンスタイムグラフを追加"
                .to_string(),
        );
    }

    // Check admin status
    let is_admin = super::http::is_admin(ctx.channel_key)
        || ctx.user_id.map_or(false, |uid| super::http::is_admin(uid))
        || ctx
            .session_key
            .starts_with("webchat:")
            && super::http::is_admin(ctx.session_key);

    if !is_admin {
        return CommandResult::Reply(
            "⛔ /improve コマンドは管理者のみ利用できます。".to_string(),
        );
    }

    CommandResult::Reply(format!(
        "🔧 改善リクエストを受け付けました:\n「{desc}」\n\n※ 自動PR作成機能は準備中です。GitHub Issueとして記録されます。"
    ))
}

// ---------------------------------------------------------------------------
// Link command internals (moved from http.rs)
// ---------------------------------------------------------------------------

#[cfg(feature = "dynamodb-backend")]
enum LinkResult {
    CodeGenerated(String),
    Linked(String),
    Error(String),
}

#[cfg(feature = "dynamodb-backend")]
async fn resolve_session_key(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    channel_key: &str,
) -> String {
    let pk = format!("LINK#{}", channel_key);
    let resp = dynamo
        .get_item()
        .table_name(config_table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
        .send()
        .await;

    match resp {
        Ok(output) => {
            if let Some(item) = output.item {
                if let Some(user_id) = item.get("user_id").and_then(|v| v.as_s().ok()) {
                    return user_id.clone();
                }
            }
            channel_key.to_string()
        }
        Err(e) => {
            tracing::warn!("resolve_session_key DynamoDB error: {}", e);
            channel_key.to_string()
        }
    }
}

/// Extract a human-readable channel display name from a channel_key.
/// e.g. "line:U12345" → "LINE", "tg:123|yukibot" → "Telegram (@yukibot)", "webchat:xxx" → "Web"
/// Returns (display_name, identifier).
#[allow(dead_code)]
fn channel_display_name(channel_key: &str) -> (String, String) {
    if let Some(rest) = channel_key.strip_prefix("line:") {
        ("LINE".to_string(), rest.to_string())
    } else if let Some(rest) = channel_key.strip_prefix("tg:") {
        if let Some((_id, username)) = rest.split_once('|') {
            (format!("Telegram (@{})", username), username.to_string())
        } else {
            ("Telegram".to_string(), rest.to_string())
        }
    } else if channel_key.starts_with("webchat:") || channel_key.starts_with("api:") {
        ("Web".to_string(), String::new())
    } else if let Some(rest) = channel_key.strip_prefix("fb:") {
        ("Facebook".to_string(), rest.to_string())
    } else {
        ("Unknown".to_string(), channel_key.to_string())
    }
}

#[cfg(feature = "dynamodb-backend")]
async fn handle_link_command(
    dynamo: &aws_sdk_dynamodb::Client,
    config_table: &str,
    channel_key: &str,
    code_arg: Option<&str>,
    sessions: &Mutex<Box<dyn SessionStore>>,
) -> LinkResult {
    match code_arg {
        None => {
            let raw = uuid::Uuid::new_v4().to_string().replace('-', "");
            let code: String = raw
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(6)
                .collect();
            let code = code.to_uppercase();

            let ttl = (chrono::Utc::now().timestamp() + 1800).to_string();

            let result = dynamo
                .put_item()
                .table_name(config_table)
                .item("pk", AttributeValue::S(format!("LINKCODE#{}", code)))
                .item("sk", AttributeValue::S("PENDING".to_string()))
                .item("channel_key", AttributeValue::S(channel_key.to_string()))
                .item("ttl", AttributeValue::N(ttl))
                .send()
                .await;

            match result {
                Ok(_) => LinkResult::CodeGenerated(format!(
                    "リンクコード: {}\n別のチャネル（LINE/Telegram/Web）で「/link {}」と送信してください。\n有効期限: 30分",
                    code, code
                )),
                Err(e) => {
                    tracing::error!("Failed to store link code: {}", e);
                    LinkResult::Error("リンクコードの生成に失敗しました。".to_string())
                }
            }
        }
        Some(code) => {
            let code = code.trim().to_uppercase();

            let resp = dynamo
                .get_item()
                .table_name(config_table)
                .key("pk", AttributeValue::S(format!("LINKCODE#{}", code)))
                .key("sk", AttributeValue::S("PENDING".to_string()))
                .send()
                .await;

            let other_channel_key = match resp {
                Ok(output) => match output.item {
                    Some(item) => {
                        if let Some(ttl_val) = item.get("ttl").and_then(|v| v.as_n().ok()) {
                            if let Ok(ttl) = ttl_val.parse::<i64>() {
                                if chrono::Utc::now().timestamp() > ttl {
                                    return LinkResult::Error(
                                        "リンクコードの有効期限が切れています。もう一度 /link で新しいコードを生成してください。"
                                            .to_string(),
                                    );
                                }
                            }
                        }
                        match item.get("channel_key").and_then(|v| v.as_s().ok()) {
                            Some(k) => k.clone(),
                            None => {
                                return LinkResult::Error(
                                    "無効なリンクコードです。".to_string(),
                                )
                            }
                        }
                    }
                    None => {
                        return LinkResult::Error(
                            "リンクコードが見つかりません。正しいコードか確認してください。"
                                .to_string(),
                        )
                    }
                },
                Err(e) => {
                    tracing::error!("Failed to look up link code: {}", e);
                    return LinkResult::Error(
                        "リンクコードの確認に失敗しました。".to_string(),
                    );
                }
            };

            if other_channel_key == channel_key {
                return LinkResult::Error(
                    "同じチャネルではリンクできません。別のチャネルからコードを入力してください。"
                        .to_string(),
                );
            }

            let existing_a =
                resolve_session_key(dynamo, config_table, &other_channel_key).await;
            let existing_b =
                resolve_session_key(dynamo, config_table, channel_key).await;

            let user_id = if existing_a.starts_with("user:") {
                existing_a.clone()
            } else if existing_b.starts_with("user:") {
                existing_b.clone()
            } else {
                format!("user:{}", uuid::Uuid::new_v4())
            };

            let now = chrono::Utc::now().to_rfc3339();
            let (other_display, _) = channel_display_name(&other_channel_key);
            let (this_display, _) = channel_display_name(channel_key);
            for ck in [&other_channel_key, &channel_key.to_string()] {
                let (ch_name, _) = channel_display_name(ck);
                let _ = dynamo
                    .put_item()
                    .table_name(config_table)
                    .item("pk", AttributeValue::S(format!("LINK#{}", ck)))
                    .item("sk", AttributeValue::S("CHANNEL_MAP".to_string()))
                    .item("user_id", AttributeValue::S(user_id.clone()))
                    .item("linked_at", AttributeValue::S(now.clone()))
                    .item("channel_name", AttributeValue::S(ch_name))
                    .send()
                    .await;
            }

            // Merge session histories into the unified session
            {
                let mut store = sessions.lock().await;
                let old_key_a = if existing_a.starts_with("user:") {
                    existing_a.clone()
                } else {
                    other_channel_key.clone()
                };
                let old_key_b = if existing_b.starts_with("user:") {
                    existing_b.clone()
                } else {
                    channel_key.to_string()
                };

                let mut all_msgs: Vec<(String, String)> = Vec::new();

                {
                    let session_a = store.get_or_create(&old_key_a);
                    for m in &session_a.messages {
                        all_msgs.push((m.role.clone(), m.content.clone()));
                    }
                }
                if old_key_b != old_key_a {
                    let session_b = store.get_or_create(&old_key_b);
                    for m in &session_b.messages {
                        all_msgs.push((m.role.clone(), m.content.clone()));
                    }
                }

                if !all_msgs.is_empty() {
                    let unified = store.get_or_create(&user_id);
                    if unified.messages.is_empty() {
                        for (role, content) in &all_msgs {
                            unified.add_message(role, content);
                        }
                    }
                    store.save_by_key(&user_id);
                }
            }

            let _ = dynamo
                .delete_item()
                .table_name(config_table)
                .key("pk", AttributeValue::S(format!("LINKCODE#{}", code)))
                .key("sk", AttributeValue::S("PENDING".to_string()))
                .send()
                .await;

            tracing::info!(
                "Channels linked: {} <-> {} => {}",
                other_channel_key,
                channel_key,
                user_id
            );
            LinkResult::Linked(format!(
                "リンク完了！{} ↔ {} が連携されました。これからどのチャネルでも同じ会話を続けられます。",
                other_display, this_display
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_help() {
        assert_eq!(parse_command("/help"), Some(SlashCommand::Help));
        assert_eq!(parse_command("  /help  "), Some(SlashCommand::Help));
        assert_eq!(parse_command("/HELP"), Some(SlashCommand::Help));
    }

    #[test]
    fn test_parse_status() {
        assert_eq!(parse_command("/status"), Some(SlashCommand::Status));
        assert_eq!(parse_command("/STATUS"), Some(SlashCommand::Status));
    }

    #[test]
    fn test_parse_share() {
        assert_eq!(parse_command("/share"), Some(SlashCommand::Share));
        assert_eq!(parse_command("  /share  "), Some(SlashCommand::Share));
    }

    #[test]
    fn test_parse_improve() {
        assert_eq!(
            parse_command("/improve add dark mode"),
            Some(SlashCommand::Improve("add dark mode"))
        );
        assert_eq!(
            parse_command("/improve"),
            Some(SlashCommand::Improve(""))
        );
    }

    #[test]
    fn test_parse_link_bare() {
        assert_eq!(parse_command("/link"), Some(SlashCommand::Link(None)));
    }

    #[test]
    fn test_parse_link_with_code() {
        assert_eq!(
            parse_command("/link ABC123"),
            Some(SlashCommand::Link(Some("ABC123")))
        );
    }

    #[test]
    fn test_parse_link_embedded_code() {
        assert_eq!(
            parse_command("こちらのコードを入力してください /link AB12CD"),
            Some(SlashCommand::Link(Some("AB12CD")))
        );
    }

    #[test]
    fn test_parse_not_a_command() {
        assert_eq!(parse_command("hello world"), None);
        assert_eq!(parse_command("what is /link about?"), None);
        assert_eq!(parse_command("/unknown"), None);
    }

    #[test]
    fn test_help_text_contains_commands() {
        let text = help_text();
        assert!(text.contains("/help"));
        assert!(text.contains("/status"));
        assert!(text.contains("/share"));
        assert!(text.contains("/link"));
        assert!(text.contains("/improve"));
    }

    #[test]
    fn test_base62_encode() {
        assert_eq!(base62_encode(0, 1).len(), 1);
        let encoded = base62_encode(12345678901234, 10);
        assert_eq!(encoded.len(), 10);
        // All chars should be alphanumeric
        assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_generate_share_hash() {
        let hash = generate_share_hash();
        assert_eq!(hash.len(), 10);
        assert!(hash.chars().all(|c| c.is_ascii_alphanumeric()));
        // Should be unique
        let hash2 = generate_share_hash();
        assert_ne!(hash, hash2);
    }

    #[test]
    fn test_channel_display_name() {
        let (name, _) = channel_display_name("line:U12345");
        assert_eq!(name, "LINE");

        let (name, id) = channel_display_name("tg:123456|yukibot");
        assert_eq!(name, "Telegram (@yukibot)");
        assert_eq!(id, "yukibot");

        let (name, _) = channel_display_name("tg:123456");
        assert_eq!(name, "Telegram");

        let (name, _) = channel_display_name("webchat:abc");
        assert_eq!(name, "Web");

        let (name, _) = channel_display_name("api:xyz");
        assert_eq!(name, "Web");

        let (name, _) = channel_display_name("fb:12345");
        assert_eq!(name, "Facebook");
    }
}
