use std::sync::Arc;
use std::collections::HashMap;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, ClearType};
use crossterm::ExecutableCommand;
use indicatif::{ProgressBar, ProgressStyle};

use nanobot_core::bus::MessageBus;
use nanobot_core::config::{self, Config};
use nanobot_core::provider;

#[derive(Parser)]
#[command(
    name = "chatweb",
    about = format!("{} chatweb - AI Assistant by chatweb.ai", nanobot_core::LOGO),
    version = nanobot_core::VERSION,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Voice-first interactive mode (default)
    Voice {
        /// API endpoint
        #[arg(long, default_value = "https://chatweb.ai/api/v1/chat")]
        api: String,
        /// Sync with a Web/LINE/Telegram session ID
        #[arg(long)]
        sync: Option<String>,
    },
    /// Chat with AI via chatweb.ai (no config needed)
    Chat {
        /// Message to send (or omit for interactive mode)
        message: Vec<String>,
        /// API endpoint
        #[arg(long, default_value = "https://chatweb.ai/api/v1/chat")]
        api: String,
        /// Sync with a Web/LINE/Telegram session ID
        #[arg(long)]
        sync: Option<String>,
    },
    /// Link CLI with Web/LINE/Telegram session
    Link {
        /// Web session ID to link with (e.g. api:xxxx-xxxx)
        session_id: Option<String>,
    },
    /// Initialize chatweb configuration and workspace
    Onboard,
    /// Interact with the agent directly
    Agent {
        /// Message to send to the agent
        #[arg(short, long)]
        message: Option<String>,
        /// Session ID
        #[arg(short, long, default_value = "cli:default")]
        session: String,
    },
    /// Start the chatweb gateway
    Gateway {
        /// Gateway port
        #[arg(short, long, default_value_t = 18790)]
        port: u16,
        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
        /// Start HTTP API server
        #[arg(long)]
        http: bool,
        /// HTTP server port (default: 3000)
        #[arg(long, default_value_t = 3000)]
        http_port: u16,
        /// Require Bearer token authentication
        #[arg(long)]
        auth: bool,
    },
    /// Show chatweb status
    Status,
    /// Manage channels
    Channels {
        #[command(subcommand)]
        command: ChannelCommands,
    },
    /// Run background daemon (device monitoring + heartbeat)
    Daemon {
        /// Heartbeat interval in seconds
        #[arg(long, default_value_t = 60)]
        interval: u64,
        /// API endpoint
        #[arg(long, default_value = "https://chatweb.ai")]
        api: String,
    },
    /// Manage scheduled tasks
    Cron {
        #[command(subcommand)]
        command: CronCommands,
    },
    /// Earn credits by running local LLM inference for other users
    Earn {
        /// Model to serve (qwen3-0.6b, qwen3-1.7b, qwen3-4b)
        #[arg(long, default_value = "qwen3-1.7b")]
        model: String,
        /// API endpoint
        #[arg(long, default_value = "https://chatweb.ai")]
        api: String,
    },
    /// Generate a new API token for Gateway authentication
    GenToken,
}

#[derive(Subcommand)]
enum ChannelCommands {
    /// Show channel status
    Status,
}

#[derive(Subcommand)]
enum CronCommands {
    /// List scheduled jobs
    List {
        /// Include disabled jobs
        #[arg(short, long)]
        all: bool,
    },
    /// Add a scheduled job
    Add {
        /// Job name
        #[arg(short, long)]
        name: String,
        /// Message for agent
        #[arg(short, long)]
        message: String,
        /// Run every N seconds
        #[arg(short, long)]
        every: Option<u64>,
        /// Cron expression
        #[arg(short, long)]
        cron: Option<String>,
    },
    /// Remove a scheduled job
    Remove {
        /// Job ID
        job_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("nanobot=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            // Default to Voice mode when no subcommand specified
            cmd_voice("https://chatweb.ai/api/v1/chat".to_string(), None).await?
        }
        Some(Commands::Voice { api, sync }) => cmd_voice(api, sync).await?,
        Some(Commands::Chat { message, api, sync }) => cmd_chat(message, api, sync).await?,
        Some(Commands::Link { session_id }) => cmd_link(session_id).await?,
        Some(Commands::Onboard) => cmd_onboard()?,
        Some(Commands::Agent { message, session }) => cmd_agent(message, session).await?,
        Some(Commands::Gateway { port, verbose, http, http_port, auth }) => cmd_gateway(port, verbose, http, http_port, auth).await?,
        Some(Commands::Daemon { interval, api }) => cmd_daemon(interval, api).await?,
        Some(Commands::Status) => cmd_status()?,
        Some(Commands::Channels { command }) => match command {
            ChannelCommands::Status => cmd_channels_status()?,
        },
        Some(Commands::Cron { command }) => match command {
            CronCommands::List { all } => cmd_cron_list(all)?,
            CronCommands::Add {
                name,
                message,
                every,
                cron,
            } => cmd_cron_add(name, message, every, cron)?,
            CronCommands::Remove { job_id } => cmd_cron_remove(job_id)?,
        },
        Some(Commands::Earn { model, api }) => cmd_earn(model, api).await?,
        Some(Commands::GenToken) => cmd_gen_token(),
    }

    Ok(())
}

// ====== Commands ======

/// Get or create CLI session ID.
fn get_cli_session_id() -> Result<String> {
    let data_dir = config::get_data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let session_file = data_dir.join("cli_session_id");
    if session_file.exists() {
        Ok(std::fs::read_to_string(&session_file)?.trim().to_string())
    } else {
        let id = format!("cli:{}", uuid::Uuid::new_v4());
        std::fs::write(&session_file, &id)?;
        Ok(id)
    }
}

/// Display welcome banner in Claude Code style
fn show_welcome_banner(session_id: &str, synced: bool, authenticated: bool) {
    // Get current directory
    let current_dir = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "~".to_string());

    // ASCII art banner with nanobot branding
    println!("\x1b[1;36m ▐▛█▙▟█▛▌\x1b[0m   chatweb v{}", nanobot_core::VERSION);
    println!("\x1b[1;36m▝▜█████▛▘\x1b[0m  Voice-First AI Assistant");
    println!("\x1b[1;36m  ▘▘ ▝▝\x1b[0m    {}", current_dir);
    println!();

    // Status indicators
    if synced {
        println!("\x1b[32m  ✓ Synced with Web session\x1b[0m");
    }
    if authenticated {
        println!("\x1b[32m  ✓ Authenticated\x1b[0m");
    }
    if synced || authenticated {
        println!();
    }

    // Session info
    println!("\x1b[2m  Session: {}\x1b[0m", session_id);
    println!();

    // Commands - Mobile-friendly
    println!("\x1b[2m  📱 スマホ向けコマンド:\x1b[0m");
    println!("\x1b[2m    ? or /q   クイックメニュー\x1b[0m");
    println!("\x1b[2m    /h        ヘルプ\x1b[0m");
    println!("\x1b[2m    /m        よく使うフレーズ\x1b[0m");
    println!("\x1b[2m    1-5       数字でクイックアクション\x1b[0m");
}


/// Check if input is a mobile-friendly easter egg pattern and return the code name
fn check_mobile_easter_egg(input: &str) -> Option<&'static str> {
    match input {
        "❤️❤️❤️" | "💕💕💕" | "😊😊😊" => Some("LOVE"),
        "🎉🎉🎉" | "🎊🎊🎊" | "🎁🎁🎁" => Some("CELEBRATE"),
        "✨✨✨" | "⭐⭐⭐" | "🌟🌟🌟" => Some("STAR"),
        "🎮🎮🎮" | "🎯🎯🎯" => Some("GAMER"),
        "!!!" => Some("ENERGY"),
        "???" => Some("MYSTERY"),
        "..." => Some("THINKING"),
        "123" | "1234" | "321" => Some("NUMBER"),
        "love" | "LOVE" => Some("WORDLOVE"),
        "ありがとう❤️" => Some("THANKS"),
        "すごい！！！" => Some("AMAZING"),
        _ => None,
    }
}

/// Show mobile easter egg animation
fn show_mobile_easter_egg_animation(input: &str, credits_granted: i64, credits_remaining: i64) {
    use std::io::Write;

    println!();

    // Different animations based on pattern type
    let (emoji, title, color) = if input.contains('❤') || input.contains('💕') {
        ("💕", "LOVE BONUS", "35") // Magenta
    } else if input.contains('🎉') || input.contains('🎊') || input.contains('🎁') {
        ("🎉", "CELEBRATION BONUS", "33") // Yellow
    } else if input.contains('✨') || input.contains('⭐') || input.contains('🌟') {
        ("✨", "STAR BONUS", "36") // Cyan
    } else if input.contains('🎮') || input.contains('🎯') {
        ("🎮", "GAMER BONUS", "35") // Magenta
    } else if input == "..." {
        ("💭", "THINKING BONUS", "34") // Blue
    } else if input == "!!!" {
        ("🔥", "ENERGY BONUS", "31") // Red
    } else if input.starts_with(char::is_numeric) {
        ("🔢", "NUMBER BONUS", "32") // Green
    } else {
        ("🎁", "SECRET BONUS", "35") // Magenta
    };

    println!("\x1b[1;{}m╔═══════════════════════════════════════╗\x1b[0m", color);
    println!("\x1b[1;{}m║                                       ║\x1b[0m", color);
    println!("\x1b[1;{}m║     {}  {} {}     ║\x1b[0m", color, emoji, title, emoji);
    println!("\x1b[1;{}m║                                       ║\x1b[0m", color);
    println!("\x1b[1;{}m║        スマホで発見おめでとう！        ║\x1b[0m", color);
    println!("\x1b[1;{}m║                                       ║\x1b[0m", color);
    println!("\x1b[1;{}m╚═══════════════════════════════════════╝\x1b[0m", color);
    println!();

    // Animated credit count-up
    let steps = 15;
    let increment = credits_granted / steps;
    for i in 1..=steps {
        let current = if i == steps { credits_granted } else { increment * i };
        print!("\r\x1b[1;33m  {} +{} credits\x1b[0m", emoji, current);
        std::io::stdout().flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(40));
    }

    println!();
    println!("\x1b[1;32m  {} New balance: {} credits\x1b[0m", emoji, credits_remaining);
    println!();
}

/// Show omikuji (fortune) animation
fn show_omikuji_animation(fortune: &str, credits_granted: i64, credits_remaining: i64, message: &str) {
    use std::io::Write;

    // Spinning animation
    let spinner = ["🎴", "🎋", "🎐", "🎎"];
    for _ in 0..10 {
        for frame in &spinner {
            print!("\r  {} おみくじを引いています...  ", frame);
            std::io::stdout().flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    print!("\r                                            \r");
    std::io::stdout().flush().unwrap();

    println!();

    // Fortune box with color based on result
    let (color, emoji) = match fortune {
        "大吉" => ("31", "🌸"),  // Red
        "吉" => ("33", "🌼"),    // Yellow
        "中吉" => ("32", "🍀"),  // Green
        "小吉" => ("36", "☘️"),   // Cyan
        _ => ("37", "🌿"),       // White
    };

    println!("\x1b[1;{}m┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓\x1b[0m", color);
    println!("\x1b[1;{}m┃                                   ┃\x1b[0m", color);
    println!("\x1b[1;{}m┃        {}  本日の運勢  {}        ┃\x1b[0m", color, emoji, emoji);
    println!("\x1b[1;{}m┃                                   ┃\x1b[0m", color);
    println!("\x1b[1;{}m┃             【 {} 】             ┃\x1b[0m", color, fortune);
    println!("\x1b[1;{}m┃                                   ┃\x1b[0m", color);
    println!("\x1b[1;{}m┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛\x1b[0m", color);
    println!();

    // Fortune message
    println!("\x1b[2m  {}\x1b[0m", message);
    println!();

    // Credit count-up
    if credits_granted > 0 {
        let steps = 10;
        let increment = credits_granted / steps;
        for i in 1..=steps {
            let current = if i == steps { credits_granted } else { increment * i };
            print!("\r\x1b[1;33m  {} +{} credits\x1b[0m", emoji, current);
            std::io::stdout().flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        println!();
        println!("\x1b[1;32m  {} New balance: {} credits\x1b[0m", emoji, credits_remaining);
    }
    println!();
}

/// Redeem easter egg code via API
async fn redeem_easter_egg(client: &reqwest::Client, api_url: &str, code: &str, session_id: &str, auth_token: Option<&str>) -> Result<serde_json::Value> {
    let redeem_url = api_url.replace("/api/v1/chat", "/api/v1/coupon/redeem");

    let body = serde_json::json!({
        "code": code,
        "session_id": session_id,
    });

    let mut req = client.post(&redeem_url).json(&body);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let resp = req.send().await?;
    let result: serde_json::Value = resp.json().await?;
    Ok(result)
}

/// Redeem Konami code via API (legacy)
async fn redeem_konami_code(client: &reqwest::Client, api_url: &str, session_id: &str, auth_token: Option<&str>) -> Result<serde_json::Value> {
    redeem_easter_egg(client, api_url, "KONAMI", session_id, auth_token).await
}

/// Draw omikuji (fortune) via API
async fn draw_omikuji(client: &reqwest::Client, api_url: &str, session_id: &str, auth_token: Option<&str>) -> Result<serde_json::Value> {
    let omikuji_url = api_url.replace("/api/v1/chat", "/api/v1/omikuji");

    let body = serde_json::json!({
        "session_id": session_id,
    });

    let mut req = client.post(&omikuji_url).json(&body);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let resp = req.send().await?;
    let result: serde_json::Value = resp.json().await?;
    Ok(result)
}

/// Show Konami code activation animation
fn show_konami_animation(credits_granted: i64, credits_remaining: i64) {
    use std::io::Write;

    println!();
    println!("\x1b[1;35m╔═══════════════════════════════════════╗\x1b[0m");
    println!("\x1b[1;35m║                                       ║\x1b[0m");
    println!("\x1b[1;35m║     🎮  KONAMI CODE ACTIVATED! 🎮     ║\x1b[0m");
    println!("\x1b[1;35m║                                       ║\x1b[0m");
    println!("\x1b[1;35m╚═══════════════════════════════════════╝\x1b[0m");
    println!();

    // Animated credit count-up
    let steps = 20;
    let increment = credits_granted / steps;
    for i in 1..=steps {
        let current = if i == steps { credits_granted } else { increment * i };
        print!("\r\x1b[1;33m  +{} credits\x1b[0m", current);
        std::io::stdout().flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    println!();
    println!("\x1b[1;32m  New balance: {} credits\x1b[0m", credits_remaining);
    println!();
}

/// Show quick action menu (mobile-friendly)
fn show_quick_menu() {
    println!();
    println!("\x1b[1;36m📱 クイックメニュー\x1b[0m");
    println!("\x1b[2m─────────────────────────\x1b[0m");
    println!("\x1b[1;32m1\x1b[0m. 📊 ステータス確認");
    println!("\x1b[1;32m2\x1b[0m. 🎮 コナミコード実行");
    println!("\x1b[1;32m3\x1b[0m. 🔗 セッション連携");
    println!("\x1b[1;32m4\x1b[0m. 💬 よく使うフレーズ");
    println!("\x1b[1;32m5\x1b[0m. 📋 ヘルプ表示");
    println!("\x1b[2m─────────────────────────\x1b[0m");
    println!("\x1b[2m数字を入力して選択\x1b[0m");
    println!();
}

/// Show mobile-friendly help
fn show_mobile_help() {
    println!();
    println!("\x1b[1;36m📱 スマホ向けコマンド\x1b[0m");
    println!("\x1b[2m─────────────────────────\x1b[0m");
    println!("\x1b[1;33m/q\x1b[0m または \x1b[1;33m?\x1b[0m     クイックメニュー");
    println!("\x1b[1;33m/s\x1b[0m            ステータス表示");
    println!("\x1b[1;33m/h\x1b[0m            このヘルプ");
    println!("\x1b[1;33m/c\x1b[0m            画面クリア");
    println!("\x1b[1;33m/.\x1b[0m            前回メッセージ再送");
    println!("\x1b[1;33m/m\x1b[0m            よく使うフレーズ");
    println!("\x1b[1;33m/omikuji\x1b[0m      毎日おみくじ（運試し）");
    println!("\x1b[2m─────────────────────────\x1b[0m");
    println!("\x1b[2m数字だけ: 1-5 でクイックアクション\x1b[0m");
    println!();
    println!("\x1b[2;90m💡 ヒント: 絵文字で気持ちを伝えると...\x1b[0m");
    println!();
}

/// Show frequently used phrases menu
fn show_phrases_menu() {
    println!();
    println!("\x1b[1;36m💬 よく使うフレーズ\x1b[0m");
    println!("\x1b[2m─────────────────────────\x1b[0m");
    println!("\x1b[1;32m1\x1b[0m. こんにちは！");
    println!("\x1b[1;32m2\x1b[0m. これについて詳しく教えて");
    println!("\x1b[1;32m3\x1b[0m. わかりやすく説明して");
    println!("\x1b[1;32m4\x1b[0m. 要約して");
    println!("\x1b[1;32m5\x1b[0m. ありがとう！");
    println!("\x1b[2m─────────────────────────\x1b[0m");
    println!();
}

/// Get phrase by number
fn get_phrase(num: &str) -> Option<String> {
    match num {
        "1" => Some("こんにちは！".to_string()),
        "2" => Some("これについて詳しく教えて".to_string()),
        "3" => Some("わかりやすく説明して".to_string()),
        "4" => Some("要約して".to_string()),
        "5" => Some("ありがとう！".to_string()),
        _ => None,
    }
}

/// Voice-first interactive mode with animated character.
/// Space key for push-to-talk, switch to chat mode with /chat command.
async fn cmd_voice(api_url: String, sync: Option<String>) -> Result<()> {
    let session_id = if let Some(ref sid) = sync {
        sid.clone()
    } else {
        get_cli_session_id()?
    };

    let auth_token = load_auth_token();
    let stream_url = api_url.replace("/api/v1/chat", "/api/v1/chat/stream");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap_or_default();

    // Enter raw mode for character-by-character input
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    stdout.execute(terminal::Clear(ClearType::All))?;

    // Show initial voice UI
    show_voice_ui(VoiceState::Idle, &session_id, sync.is_some(), auth_token.is_some())?;

    let mut mode = InteractionMode::Voice;
    let mut input_buffer = String::new();
    let mut listening_start = None;

    loop {
        // Poll for keyboard events
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // Check Ctrl+C globally
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }

                match mode {
                    InteractionMode::Voice => {
                        match key.code {
                            KeyCode::Char(' ') if key.kind == KeyEventKind::Press => {
                                // Start listening
                                listening_start = Some(std::time::Instant::now());
                                show_voice_ui(VoiceState::Listening, &session_id, sync.is_some(), auth_token.is_some())?;
                            }
                            KeyCode::Char(' ') if key.kind == KeyEventKind::Release => {
                                // Stop listening and process
                                if let Some(start) = listening_start {
                                    let duration = start.elapsed();
                                    show_voice_ui(VoiceState::Processing, &session_id, sync.is_some(), auth_token.is_some())?;

                                    // For now, prompt user to type (real STT would go here)
                                    terminal::disable_raw_mode()?;
                                    stdout.execute(terminal::Clear(ClearType::All))?;
                                    println!("\x1b[2m🎤 リスニング時間: {:.1}秒\x1b[0m", duration.as_secs_f32());
                                    println!("\x1b[1;33m入力してください:\x1b[0m ");

                                    use std::io::BufRead;
                                    let mut line = String::new();
                                    std::io::stdin().lock().read_line(&mut line)?;

                                    if !line.trim().is_empty() {
                                        // Send to API
                                        println!();
                                        chat_api_stream(&client, &stream_url, &api_url, line.trim(), &session_id, auth_token.as_deref()).await?;
                                        println!();
                                    }

                                    println!("\x1b[2m[スペースキー] で再度話す | [Ctrl+C] で終了 | [Enter] でチャットモードへ\x1b[0m");
                                    std::io::stdin().lock().read_line(&mut String::new())?;

                                    terminal::enable_raw_mode()?;
                                    show_voice_ui(VoiceState::Idle, &session_id, sync.is_some(), auth_token.is_some())?;
                                    listening_start = None;
                                }
                            }
                            KeyCode::Enter => {
                                // Switch to chat mode
                                mode = InteractionMode::Chat;
                                terminal::disable_raw_mode()?;
                                stdout.execute(terminal::Clear(ClearType::All))?;
                                println!("\x1b[1;36m💬 チャットモードに切り替えました\x1b[0m");
                                println!("\x1b[2m/voice でVoiceモードに戻る | Ctrl+C で終了\x1b[0m");
                                println!();
                            }
                            _ => {}
                        }
                    }
                    InteractionMode::Chat => {
                        // Handle chat mode input
                        // Check Ctrl+C first
                        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            break;
                        }

                        match key.code {
                            KeyCode::Char(c) => {
                                input_buffer.push(c);
                                print!("{}", c);
                                use std::io::Write;
                                std::io::stdout().flush()?;
                            }
                            KeyCode::Enter => {
                                println!();
                                if input_buffer.trim() == "/voice" {
                                    // Switch back to voice mode
                                    input_buffer.clear();
                                    mode = InteractionMode::Voice;
                                    terminal::enable_raw_mode()?;
                                    show_voice_ui(VoiceState::Idle, &session_id, sync.is_some(), auth_token.is_some())?;
                                } else if !input_buffer.trim().is_empty() {
                                    // Send message
                                    chat_api_stream(&client, &stream_url, &api_url, &input_buffer.trim(), &session_id, auth_token.as_deref()).await?;
                                    println!();
                                    print!("\x1b[1;33mYou:\x1b[0m ");
                                    use std::io::Write;
                                    std::io::stdout().flush()?;
                                    input_buffer.clear();
                                } else {
                                    print!("\x1b[1;33mYou:\x1b[0m ");
                                    use std::io::Write;
                                    std::io::stdout().flush()?;
                                }
                            }
                            KeyCode::Backspace => {
                                if !input_buffer.is_empty() {
                                    input_buffer.pop();
                                    print!("\x08 \x08");
                                    use std::io::Write;
                                    std::io::stdout().flush()?;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    terminal::disable_raw_mode()?;
    stdout.execute(terminal::Clear(ClearType::All))?;
    println!("\x1b[2mVoice UIを終了しました\x1b[0m");

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum InteractionMode {
    Voice,
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum VoiceState {
    Idle,
    Listening,
    Processing,
}

/// Display animated voice UI based on current state
fn show_voice_ui(state: VoiceState, session_id: &str, synced: bool, authenticated: bool) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout();

    stdout.execute(terminal::Clear(ClearType::All))?;
    stdout.execute(crossterm::cursor::MoveTo(0, 0))?;

    println!();

    // Animated character based on state
    match state {
        VoiceState::Idle => {
            println!("\x1b[1;36m     ▐▛█▙▟█▛▌\x1b[0m");
            println!("\x1b[1;36m    ▝▜█████▛▘\x1b[0m");
            println!("\x1b[1;36m      ▘▘ ▝▝\x1b[0m");
            println!();
            println!("\x1b[2m     chatweb\x1b[0m");
            println!();
            println!("\x1b[2;90m  [スペース] を押して話す\x1b[0m");
        }
        VoiceState::Listening => {
            println!("\x1b[1;32m     ▐▛█▙▟█▛▌\x1b[0m  \x1b[1;32m●\x1b[0m");
            println!("\x1b[1;32m    ▝▜█████▛▘\x1b[0m  \x1b[1;32m●\x1b[0m");
            println!("\x1b[1;32m      ▘▘ ▝▝\x1b[0m    \x1b[1;32m●\x1b[0m");
            println!();
            println!("\x1b[1;32m    🎤 リスニング中...\x1b[0m");
            println!();
            println!("\x1b[2;32m  [スペース] を離して送信\x1b[0m");
        }
        VoiceState::Processing => {
            println!("\x1b[1;33m     ▐▛█▙▟█▛▌\x1b[0m");
            println!("\x1b[1;33m    ▝▜█████▛▘\x1b[0m  \x1b[1;33m⚙\x1b[0m");
            println!("\x1b[1;33m      ▘▘ ▝▝\x1b[0m");
            println!();
            println!("\x1b[1;33m    💭 考え中...\x1b[0m");
        }
    }

    println!();
    println!();

    // Status bar at bottom
    if synced {
        println!("\x1b[2m  ✓ Synced\x1b[0m");
    }
    if authenticated {
        println!("\x1b[2m  ✓ Authenticated\x1b[0m");
    }
    println!("\x1b[2m  Session: {}\x1b[0m", &session_id[..session_id.len().min(20)]);
    println!();
    println!("\x1b[2m  [Enter] チャットモード | [Ctrl+C] 終了\x1b[0m");

    stdout.flush()?;
    Ok(())
}

/// Chat with chatweb.ai API directly — no config or API key needed.
/// Uses SSE streaming for real-time responses with tool progress.
async fn cmd_chat(message: Vec<String>, api_url: String, sync: Option<String>) -> Result<()> {
    let session_id = if let Some(ref sid) = sync {
        sid.clone()
    } else {
        get_cli_session_id()?
    };

    // Load auth token if available
    let auth_token = load_auth_token();

    // Derive streaming URL from api_url
    let stream_url = api_url.replace("/api/v1/chat", "/api/v1/chat/stream");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap_or_default();

    if message.is_empty() {
        // Interactive mode with Claude Code-style banner
        println!();
        show_welcome_banner(&session_id, sync.is_some(), auth_token.is_some());
        println!();

        let mut last_message = String::new();
        let mut in_phrase_menu = false;

        loop {
            use std::io::Write;
            print!("\x1b[1;33mYou:\x1b[0m ");
            std::io::stdout().flush()?;

            let mut input = String::new();
            if std::io::stdin().read_line(&mut input)? == 0 {
                break;
            }
            let input = input.trim();
            if input.is_empty() {
                continue;
            }

            // Check for mobile easter eggs FIRST (before other commands)
            if let Some(code) = check_mobile_easter_egg(input) {
                println!();
                print!("\x1b[2m✨ 隠しボーナス発見中...\x1b[0m");
                std::io::stdout().flush()?;

                match redeem_easter_egg(&client, &api_url, code, &session_id, auth_token.as_deref()).await {
                    Ok(result) => {
                        if result["success"].as_bool().unwrap_or(false) {
                            let granted = result["credits_granted"].as_i64().unwrap_or(1000);
                            let remaining = result["credits_remaining"].as_i64().unwrap_or(0);
                            print!("\r                              \r");
                            show_mobile_easter_egg_animation(input, granted, remaining);
                        } else if let Some(_error) = result["error"].as_str() {
                            println!("\r\x1b[2m(このパターンは既に使用済みです)\x1b[0m");
                            println!();
                        }
                    }
                    Err(e) => {
                        println!("\r\x1b[31mError: {}\x1b[0m", e);
                    }
                }
                println!();
                continue;
            }

            // Mobile-friendly shortcut commands
            match input {
                // Quick menu
                "/q" | "?" => {
                    show_quick_menu();
                    continue;
                }
                // Status
                "/s" => {
                    println!();
                    println!("\x1b[1;36m📊 ステータス\x1b[0m");
                    println!("\x1b[2m  Session: {}\x1b[0m", session_id);
                    if sync.is_some() {
                        println!("\x1b[32m  ✓ Synced\x1b[0m");
                    }
                    if auth_token.is_some() {
                        println!("\x1b[32m  ✓ Authenticated\x1b[0m");
                    }
                    println!();
                    continue;
                }
                // Mobile help
                "/h" => {
                    show_mobile_help();
                    continue;
                }
                // Clear screen
                "/c" => {
                    print!("\x1b[2J\x1b[H");
                    std::io::stdout().flush()?;
                    show_welcome_banner(&session_id, sync.is_some(), auth_token.is_some());
                    println!();
                    continue;
                }
                // Repeat last message
                "/." => {
                    if last_message.is_empty() {
                        println!("\x1b[2m前回のメッセージがありません\x1b[0m");
                        println!();
                        continue;
                    }
                    println!("\x1b[2m再送信: {}\x1b[0m", last_message);
                    println!();
                    match chat_api_stream(&client, &stream_url, &api_url, &last_message, &session_id, auth_token.as_deref()).await {
                        Ok(()) => println!(),
                        Err(e) => eprintln!("\x1b[31mError: {}\x1b[0m\n", e),
                    }
                    continue;
                }
                // Phrases menu
                "/m" => {
                    show_phrases_menu();
                    in_phrase_menu = true;
                    continue;
                }
                // Konami code
                "/konami" => {
                    println!();
                    print!("\x1b[2mActivating Konami code...\x1b[0m");
                    std::io::stdout().flush()?;

                    match redeem_konami_code(&client, &api_url, &session_id, auth_token.as_deref()).await {
                        Ok(result) => {
                            if result["success"].as_bool().unwrap_or(false) {
                                let granted = result["credits_granted"].as_i64().unwrap_or(1000);
                                let remaining = result["credits_remaining"].as_i64().unwrap_or(0);
                                print!("\r                              \r");
                                show_konami_animation(granted, remaining);
                            } else if let Some(error) = result["error"].as_str() {
                                println!("\r\x1b[31mError: {}\x1b[0m", error);
                            }
                        }
                        Err(e) => {
                            println!("\r\x1b[31mError: {}\x1b[0m", e);
                        }
                    }
                    println!();
                    continue;
                }
                // Omikuji (fortune)
                "/omikuji" | "/fortune" => {
                    println!();

                    match draw_omikuji(&client, &api_url, &session_id, auth_token.as_deref()).await {
                        Ok(result) => {
                            if result["success"].as_bool().unwrap_or(false) {
                                let fortune = result["fortune"].as_str().unwrap_or("末吉");
                                let granted = result["credits_granted"].as_i64().unwrap_or(0);
                                let remaining = result["credits_remaining"].as_i64().unwrap_or(0);
                                let message = result["message"].as_str().unwrap_or("今日も良い一日を！");
                                show_omikuji_animation(fortune, granted, remaining, message);
                            } else if let Some(error) = result["error"].as_str() {
                                println!("\x1b[33m{}\x1b[0m", error);
                                println!();
                            }
                        }
                        Err(e) => {
                            println!("\x1b[31mError: {}\x1b[0m", e);
                            println!();
                        }
                    }
                    continue;
                }
                _ => {}
            }

            // Handle numeric shortcuts
            let message_to_send = if input.len() == 1 && input.chars().all(|c| c.is_ascii_digit()) {
                let num = input;

                // If in phrase menu, use phrase
                if in_phrase_menu {
                    in_phrase_menu = false;
                    if let Some(phrase) = get_phrase(num) {
                        println!("\x1b[2m→ {}\x1b[0m", phrase);
                        phrase
                    } else {
                        println!("\x1b[31m無効な番号です\x1b[0m");
                        println!();
                        continue;
                    }
                } else {
                    // Quick action menu shortcuts
                    match num {
                        "1" => {
                            // Status
                            println!();
                            println!("\x1b[1;36m📊 ステータス\x1b[0m");
                            println!("\x1b[2m  Session: {}\x1b[0m", session_id);
                            if sync.is_some() {
                                println!("\x1b[32m  ✓ Synced\x1b[0m");
                            }
                            if auth_token.is_some() {
                                println!("\x1b[32m  ✓ Authenticated\x1b[0m");
                            }
                            println!();
                            continue;
                        }
                        "2" => {
                            // Konami
                            println!();
                            print!("\x1b[2mActivating Konami code...\x1b[0m");
                            std::io::stdout().flush()?;

                            match redeem_konami_code(&client, &api_url, &session_id, auth_token.as_deref()).await {
                                Ok(result) => {
                                    if result["success"].as_bool().unwrap_or(false) {
                                        let granted = result["credits_granted"].as_i64().unwrap_or(1000);
                                        let remaining = result["credits_remaining"].as_i64().unwrap_or(0);
                                        print!("\r                              \r");
                                        show_konami_animation(granted, remaining);
                                    } else if let Some(error) = result["error"].as_str() {
                                        println!("\r\x1b[31mError: {}\x1b[0m", error);
                                    }
                                }
                                Err(e) => {
                                    println!("\r\x1b[31mError: {}\x1b[0m", e);
                                }
                            }
                            println!();
                            continue;
                        }
                        "3" => {
                            // Link session
                            println!();
                            println!("\x1b[1;36m🔗 セッション連携\x1b[0m");
                            println!("\x1b[2m  Session ID: {}\x1b[0m", session_id);
                            println!();
                            println!("\x1b[2mWebやLINE、Telegramと連携するには:\x1b[0m");
                            println!("\x1b[2m  1. メッセージで \"/link\" と送信\x1b[0m");
                            println!("\x1b[2m  2. 表示されたコードを他のデバイスで入力\x1b[0m");
                            println!();
                            continue;
                        }
                        "4" => {
                            show_phrases_menu();
                            in_phrase_menu = true;
                            continue;
                        }
                        "5" => {
                            show_mobile_help();
                            continue;
                        }
                        _ => {
                            // Not a valid quick action, send as regular message
                            input.to_string()
                        }
                    }
                }
            } else {
                in_phrase_menu = false;
                input.to_string()
            };

            // Send message to API
            last_message = message_to_send.clone();
            println!();
            match chat_api_stream(&client, &stream_url, &api_url, &message_to_send, &session_id, auth_token.as_deref()).await {
                Ok(()) => println!(),
                Err(e) => eprintln!("\x1b[31mError: {}\x1b[0m\n", e),
            }
        }
    } else {
        // Single message mode
        let msg = message.join(" ");
        match chat_api_stream(&client, &stream_url, &api_url, &msg, &session_id, auth_token.as_deref()).await {
            Ok(()) => {}
            Err(e) => eprintln!("\x1b[31mError: {}\x1b[0m", e),
        }
    }

    Ok(())
}

/// Load auth token from ~/.nanobot/auth_token if available.
fn load_auth_token() -> Option<String> {
    let token_path = config::get_data_dir().join("auth_token");
    std::fs::read_to_string(token_path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Link CLI session with a Web/LINE/Telegram session.
async fn cmd_link(session_id: Option<String>) -> Result<()> {
    let cli_session = get_cli_session_id()?;
    let client = reqwest::Client::new();
    let api_base = "https://chatweb.ai";

    match session_id {
        Some(web_sid) => {
            // Link CLI with the given Web session by sending /link command
            // First generate a code from CLI session
            let resp = client
                .post(format!("{}/api/v1/chat", api_base))
                .json(&serde_json::json!({
                    "message": "/link",
                    "session_id": cli_session,
                }))
                .send()
                .await?;
            let body: serde_json::Value = resp.json().await?;
            let response = body["response"].as_str().unwrap_or("");

            // Extract the code
            let code = response.chars()
                .collect::<String>()
                .split_whitespace()
                .find(|w| w.len() == 6 && w.chars().all(|c| c.is_ascii_alphanumeric()))
                .map(|s| s.to_string());

            if let Some(code) = code {
                // Now link from the Web session side
                let resp2 = client
                    .post(format!("{}/api/v1/chat", api_base))
                    .json(&serde_json::json!({
                        "message": format!("/link {}", code),
                        "session_id": web_sid,
                    }))
                    .send()
                    .await?;
                let body2: serde_json::Value = resp2.json().await?;
                let result = body2["response"].as_str().unwrap_or("Link failed");
                println!("{}", result);
            } else {
                eprintln!("Failed to generate link code");
            }
        }
        None => {
            // Show CLI session ID for linking
            println!("{} CLI Session ID:", nanobot_core::LOGO);
            println!();
            println!("  {}", cli_session);
            println!();
            println!("To sync with Web: paste this ID on the chatweb.ai sync section");
            println!("To sync with Web session: chatweb link <WEB_SESSION_ID>");
            println!("To chat with synced session: chatweb chat --sync <SESSION_ID>");
        }
    }

    Ok(())
}

/// Stream a chat response via SSE, displaying progress and content in real-time.
/// Falls back to non-streaming API if SSE fails.
async fn chat_api_stream(
    client: &reqwest::Client,
    stream_url: &str,
    fallback_url: &str,
    message: &str,
    session_id: &str,
    auth_token: Option<&str>,
) -> Result<()> {
    use std::io::Write;

    let body = serde_json::json!({
        "message": message,
        "session_id": session_id,
        "channel": "cli",
        "language": "ja",
    });

    let mut req = client.post(stream_url).json(&body);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let mut resp = match req.send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            // SSE failed, try non-streaming fallback
            let status = r.status();
            tracing::debug!("Stream returned {}, falling back to non-stream", status);
            return chat_api_fallback(client, fallback_url, message, session_id, auth_token).await;
        }
        Err(e) if e.is_timeout() => {
            println!("\x1b[2m考えすぎちゃった...もう一回聞いてくれる？\x1b[0m");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let mut buf = String::new();
    let mut got_content = false;
    let mut printed_prefix = false;
    let mut tool_spinners: HashMap<String, ProgressBar> = HashMap::new();

    while let Some(chunk) = resp.chunk().await? {
        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        while let Some(newline_pos) = buf.find('\n') {
            let line = buf[..newline_pos].to_string();
            buf = buf[newline_pos + 1..].to_string();

            if !line.starts_with("data:") {
                continue;
            }
            let data = line[5..].trim();
            if data.is_empty() {
                continue;
            }

            let parsed: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Handle both single events and JSON arrays
            let events: Vec<&serde_json::Value> = if parsed.is_array() {
                parsed.as_array().unwrap().iter().collect()
            } else {
                vec![&parsed]
            };

            for evt in events {
                let evt_type = evt["type"].as_str().unwrap_or("");
                match evt_type {
                    "tool_start" => {
                        let tool = evt["tool"].as_str().unwrap_or("tool");
                        let spinner = ProgressBar::new_spinner();
                        spinner.set_style(
                            ProgressStyle::default_spinner()
                                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
                                .template("{spinner:.cyan} {msg}")
                                .unwrap()
                        );
                        spinner.set_message(format!("{}", tool));
                        spinner.enable_steady_tick(std::time::Duration::from_millis(80));
                        tool_spinners.insert(tool.to_string(), spinner);
                    }
                    "tool_result" => {
                        let tool = evt["tool"].as_str().unwrap_or("tool");
                        let ok = evt["success"].as_bool().unwrap_or(true);

                        // Finish spinner if exists
                        if let Some(spinner) = tool_spinners.remove(tool) {
                            spinner.finish_and_clear();
                        }

                        // Show result - more concise
                        if ok {
                            println!("\x1b[2m  ✓ {}\x1b[0m", tool);
                        } else {
                            // Only show details on failure
                            let summary = evt["summary"].as_str().or_else(|| evt["result"].as_str());
                            if let Some(s) = summary {
                                println!("\x1b[31m  ✗ {}: {}\x1b[0m", tool, truncate_str(s, 60));
                            } else {
                                println!("\x1b[31m  ✗ {}\x1b[0m", tool);
                            }
                        }
                    }
                    "thinking" => {
                        // More subtle thinking display - only show first 40 chars
                        let thought = evt["content"].as_str().unwrap_or("");
                        if !thought.is_empty() && thought.len() > 10 {
                            println!("\x1b[2;90m  💭 {}\x1b[0m", truncate_str(thought, 40));
                        }
                    }
                    "content_chunk" => {
                        if !printed_prefix {
                            print!("\x1b[1;36m{}\x1b[0m ", nanobot_core::LOGO);
                            printed_prefix = true;
                        }
                        let chunk_text = evt["text"].as_str().unwrap_or("");
                        print!("{}", chunk_text);
                        std::io::stdout().flush()?;
                        got_content = true;
                    }
                    "content" => {
                        if !got_content {
                            let content = evt["content"].as_str().unwrap_or("");
                            if !content.is_empty() {
                                println!("\x1b[1;36m{}\x1b[0m {}", nanobot_core::LOGO, content);
                                got_content = true;
                            }
                        } else {
                            // Streaming already printed content, just add newline
                            println!();
                        }
                        // Show credits if available - more prominent
                        if let Some(remaining) = evt["credits_remaining"].as_i64() {
                            let color = if remaining > 500 {
                                "\x1b[32m" // green
                            } else if remaining > 100 {
                                "\x1b[33m" // yellow
                            } else {
                                "\x1b[31m" // red
                            };
                            println!("{}  💳 {} credits\x1b[0m", color, remaining);
                        }
                    }
                    "error" => {
                        let msg = evt["content"].as_str().unwrap_or("Unknown error");
                        println!("\x1b[31m  Error: {}\x1b[0m", msg);
                        if evt["action"].as_str() == Some("upgrade") {
                            println!("\x1b[33m  → Upgrade at https://chatweb.ai/pricing\x1b[0m");
                        }
                    }
                    "start" | "done" => {}
                    _ => {}
                }
            }
        }
    }

    // Clean up any remaining spinners
    for (_, spinner) in tool_spinners {
        spinner.finish_and_clear();
    }

    if !got_content {
        println!("\x1b[2mレスポンスを受信できませんでした。\x1b[0m");
    }

    Ok(())
}

/// Non-streaming fallback for when SSE is unavailable.
async fn chat_api_fallback(
    client: &reqwest::Client,
    api_url: &str,
    message: &str,
    session_id: &str,
    auth_token: Option<&str>,
) -> Result<()> {
    let body = serde_json::json!({
        "message": message,
        "session_id": session_id,
        "channel": "cli",
        "language": "ja",
    });

    let mut req = client.post(api_url).json(&body);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => {
            println!("\x1b[2m考えすぎちゃった...もう一回聞いてくれる？\x1b[0m");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let body: serde_json::Value = resp.json().await?;
    let response = body["response"].as_str().unwrap_or("No response");
    println!("\x1b[1;36m{}\x1b[0m {}", nanobot_core::LOGO, response);

    if let Some(remaining) = body["credits_remaining"].as_i64() {
        println!("\x1b[2m  Credits: {}\x1b[0m", remaining);
    }

    Ok(())
}

/// Truncate a string to max length, adding "..." if truncated.
fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max - 3).collect();
        format!("{}...", truncated)
    }
}

/// Run background daemon that sends heartbeats to chatweb.ai.
async fn cmd_daemon(interval: u64, api_base: String) -> Result<()> {
    let session_id = get_cli_session_id()?;
    let client = reqwest::Client::new();
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    println!("{} chatweb.ai daemon starting", nanobot_core::LOGO);
    println!("  Session: {}", session_id);
    println!("  Hostname: {}", hostname);
    println!("  Heartbeat interval: {}s", interval);
    println!();

    loop {
        let heartbeat = serde_json::json!({
            "session_id": session_id,
            "hostname": hostname,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "uptime_secs": get_system_uptime(),
        });

        match client
            .post(format!("{}/api/v1/devices/heartbeat", api_base))
            .json(&heartbeat)
            .send()
            .await
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    tracing::debug!("Heartbeat sent successfully");
                } else {
                    tracing::warn!("Heartbeat failed: {}", resp.status());
                }
            }
            Err(e) => {
                tracing::warn!("Heartbeat error: {}", e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
    }
}

/// Get system uptime in seconds (best effort).
fn get_system_uptime() -> u64 {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("sysctl").arg("-n").arg("kern.boottime").output() {
            let s = String::from_utf8_lossy(&output.stdout);
            // Parse "{ sec = 1234567890, usec = 0 } ..."
            if let Some(sec_start) = s.find("sec = ") {
                let rest = &s[sec_start + 6..];
                if let Some(comma) = rest.find(',') {
                    if let Ok(boot_sec) = rest[..comma].trim().parse::<u64>() {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        return now.saturating_sub(boot_sec);
                    }
                }
            }
        }
        0
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next().map(|s| s.to_string()))
            .and_then(|s| s.parse::<f64>().ok())
            .map(|f| f as u64)
            .unwrap_or(0)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        0
    }
}

fn cmd_onboard() -> Result<()> {
    let config_path = config::get_config_path();

    if config_path.exists() {
        println!("Config already exists at {}", config_path.display());
        println!("Delete it first to re-onboard.");
        return Ok(());
    }

    let cfg = Config::default();
    config::save_config(&cfg, None)?;
    println!("{} Created config at {}", nanobot_core::LOGO, config_path.display());

    let workspace = cfg.workspace_path();
    std::fs::create_dir_all(&workspace)?;
    println!("{} Created workspace at {}", nanobot_core::LOGO, workspace.display());

    // Create default bootstrap files
    create_workspace_templates(&workspace)?;

    println!("\n{} chatweb is ready!", nanobot_core::LOGO);
    println!("\nNext steps:");
    println!("  1. Add your API key to ~/.nanobot/config.json");
    println!("     Get one at: https://openrouter.ai/keys");
    println!("  2. Chat: chatweb agent -m \"Hello!\"");
    Ok(())
}

fn create_workspace_templates(workspace: &std::path::Path) -> Result<()> {
    let templates = [
        ("AGENTS.md", "# Agent Instructions\n\nYou are a helpful AI assistant. Be concise, accurate, and friendly.\n\n## Guidelines\n\n- Always explain what you're doing before taking actions\n- Ask for clarification when the request is ambiguous\n- Use tools to help accomplish tasks\n- Remember important information in your memory files\n"),
        ("SOUL.md", "# Soul\n\nI am chatweb, a voice-first AI assistant by chatweb.ai.\n\n## Personality\n\n- Helpful and friendly\n- Concise and to the point\n- Curious and eager to learn\n\n## Values\n\n- Accuracy over speed\n- User privacy and safety\n- Transparency in actions\n"),
        ("USER.md", "# User\n\nInformation about the user goes here.\n\n## Preferences\n\n- Communication style: (casual/formal)\n- Timezone: (your timezone)\n- Language: (your preferred language)\n"),
    ];

    for (filename, content) in &templates {
        let file_path = workspace.join(filename);
        if !file_path.exists() {
            std::fs::write(&file_path, content)?;
            println!("  Created {}", filename);
        }
    }

    let memory_dir = workspace.join("memory");
    std::fs::create_dir_all(&memory_dir)?;
    let memory_file = memory_dir.join("MEMORY.md");
    if !memory_file.exists() {
        std::fs::write(
            &memory_file,
            "# Long-term Memory\n\nThis file stores important information that should persist across sessions.\n\n## User Information\n\n(Important facts about the user)\n\n## Preferences\n\n(User preferences learned over time)\n\n## Important Notes\n\n(Things to remember)\n",
        )?;
        println!("  Created memory/MEMORY.md");
    }

    Ok(())
}

async fn cmd_agent(message: Option<String>, session_id: String) -> Result<()> {
    let cfg = config::load_config(None);

    let model = cfg.agents.defaults.model.clone();
    let is_bedrock = model.starts_with("bedrock/");
    let api_key = cfg.get_api_key(None);

    if api_key.is_none() && !is_bedrock {
        eprintln!("Error: No API key configured.");
        eprintln!("Set one in ~/.nanobot/config.json under providers");
        std::process::exit(1);
    }

    let api_key_str = api_key.unwrap_or("").to_string();
    let api_base = cfg.get_api_base(None).map(|s| s.to_string());

    let llm_provider: Arc<dyn provider::LlmProvider> = Arc::from(provider::create_provider(
        &api_key_str,
        api_base.as_deref(),
        &model,
    ));

    let bus = MessageBus::new(256);

    let brave_api_key = if cfg.tools.web.search.api_key.is_empty() {
        None
    } else {
        Some(cfg.tools.web.search.api_key.clone())
    };

    let mut agent = nanobot_core::agent::AgentLoop::new(
        bus,
        llm_provider,
        cfg.workspace_path(),
        Some(model),
        cfg.agents.defaults.max_tool_iterations,
        brave_api_key,
        cfg.tools.exec_config.clone(),
        cfg.tools.restrict_to_workspace,
        None,
    );

    if let Some(msg) = message {
        // Single message mode
        let response = agent
            .process_direct(&msg, &session_id, "cli", "direct")
            .await?;
        println!("\n{} {}", nanobot_core::LOGO, response);
    } else {
        // Interactive mode
        println!(
            "{} Interactive mode (Ctrl+C to exit)\n",
            nanobot_core::LOGO
        );

        loop {
            use std::io::Write;
            print!("You: ");
            std::io::stdout().flush()?;

            let mut input = String::new();
            if std::io::stdin().read_line(&mut input)? == 0 {
                break;
            }

            let input = input.trim();
            if input.is_empty() {
                continue;
            }

            let response = agent
                .process_direct(input, &session_id, "cli", "direct")
                .await?;
            println!("\n{} {}\n", nanobot_core::LOGO, response);
        }
    }

    Ok(())
}

#[allow(unused_variables)]
async fn cmd_gateway(port: u16, verbose: bool, http: bool, http_port: u16, auth: bool) -> Result<()> {
    if verbose {
        // Re-init with debug level
        // Already handled by env filter
    }

    let cfg = config::load_config_from_env();

    #[cfg(feature = "http-api")]
    if http {
        use nanobot_core::service::http::{serve_with_auth, AppState};
        use nanobot_core::session::file_store::FileSessionStore;

        let workspace = cfg.workspace_path();
        let state = std::sync::Arc::new(AppState::with_provider(
            cfg.clone(),
            Box::new(FileSessionStore::new(&workspace)),
        ));

        let addr = format!("0.0.0.0:{}", http_port);
        println!(
            "{} Starting chatweb HTTP API on {}...",
            nanobot_core::LOGO, addr
        );
        if auth {
            println!("  Authentication: ENABLED");
        } else {
            println!("  Authentication: disabled (use --auth to enable)");
        }

        // Run HTTP server and gateway concurrently
        let http_handle = tokio::spawn(async move {
            if let Err(e) = serve_with_auth(&addr, state, auth).await {
                eprintln!("HTTP server error: {}", e);
            }
        });

        let gateway_handle = tokio::spawn(async move {
            if let Err(e) = nanobot_core::service::gateway::run_gateway(cfg).await {
                eprintln!("Gateway error: {}", e);
            }
        });

        tokio::select! {
            _ = http_handle => {},
            _ = gateway_handle => {},
        }
        return Ok(());
    }

    #[cfg(not(feature = "http-api"))]
    if http {
        eprintln!("HTTP API not available. Rebuild with: cargo build --features http-api");
        std::process::exit(1);
    }

    println!(
        "{} Starting chatweb gateway on port {}...",
        nanobot_core::LOGO, port
    );

    nanobot_core::service::gateway::run_gateway(cfg).await
}

fn cmd_status() -> Result<()> {
    let config_path = config::get_config_path();
    let cfg = config::load_config(None);
    let workspace = cfg.workspace_path();

    println!("{} chatweb Status\n", nanobot_core::LOGO);

    let config_exists = config_path.exists();
    println!(
        "Config: {} {}",
        config_path.display(),
        if config_exists { "✓" } else { "✗" }
    );
    println!(
        "Workspace: {} {}",
        workspace.display(),
        if workspace.exists() { "✓" } else { "✗" }
    );

    if config_exists {
        println!("Model: {}", cfg.agents.defaults.model);
        println!(
            "OpenRouter API: {}",
            if cfg.providers.openrouter.api_key.is_empty() {
                "not set"
            } else {
                "✓"
            }
        );
        println!(
            "Anthropic API: {}",
            if cfg.providers.anthropic.api_key.is_empty() {
                "not set"
            } else {
                "✓"
            }
        );
        println!(
            "OpenAI API: {}",
            if cfg.providers.openai.api_key.is_empty() {
                "not set"
            } else {
                "✓"
            }
        );
        println!(
            "Gemini API: {}",
            if cfg.providers.gemini.api_key.is_empty() {
                "not set"
            } else {
                "✓"
            }
        );
        let vllm_status = if let Some(ref base) = cfg.providers.vllm.api_base {
            format!("✓ {}", base)
        } else {
            "not set".to_string()
        };
        println!("vLLM/Local: {}", vllm_status);
    }

    Ok(())
}

fn cmd_gen_token() {
    let token = uuid::Uuid::new_v4().to_string();
    println!("{} Generated Gateway API Token:\n", nanobot_core::LOGO);
    println!("  {}\n", token);
    println!("Add this token to your config:");
    println!("  1. Edit ~/.nanobot/config.json");
    println!("  2. Add token to \"gateway.apiTokens\" array");
    println!("  3. Or set GATEWAY_API_TOKENS environment variable\n");
    println!("Example:");
    println!("  GATEWAY_API_TOKENS=\"{}\" chatweb gateway --http --auth", token);
}

fn cmd_channels_status() -> Result<()> {
    let cfg = config::load_config(None);

    println!("Channel Status\n");
    println!(
        "  WhatsApp:  {} ({})",
        if cfg.channels.whatsapp.enabled {
            "✓"
        } else {
            "✗"
        },
        cfg.channels.whatsapp.bridge_url
    );
    println!(
        "  Telegram:  {} ({})",
        if cfg.channels.telegram.enabled {
            "✓"
        } else {
            "✗"
        },
        if cfg.channels.telegram.token.is_empty() {
            "not configured"
        } else {
            "configured"
        }
    );
    println!(
        "  Discord:   {} ({})",
        if cfg.channels.discord.enabled {
            "✓"
        } else {
            "✗"
        },
        cfg.channels.discord.gateway_url
    );
    println!(
        "  Feishu:    {} ({})",
        if cfg.channels.feishu.enabled {
            "✓"
        } else {
            "✗"
        },
        if cfg.channels.feishu.app_id.is_empty() {
            "not configured"
        } else {
            "configured"
        }
    );
    println!(
        "  LINE:      {} ({})",
        if cfg.channels.line.enabled {
            "✓"
        } else {
            "✗"
        },
        if cfg.channels.line.channel_access_token.is_empty() {
            "not configured"
        } else {
            "configured"
        }
    );

    Ok(())
}

/// Earn credits by running local LLM inference as a worker.
async fn cmd_earn(model: String, api_base: String) -> Result<()> {
    let session_id = get_cli_session_id()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(35))
        .build()
        .unwrap_or_default();
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let credits_per_req: u32 = match model.as_str() {
        "qwen3-0.6b" => 1,
        "qwen3-1.7b" => 2,
        "qwen3-4b" => 5,
        _ => 2,
    };

    println!("{} chatweb earn — Compute Provider", nanobot_core::LOGO);
    println!("  Model: {} ({} credits/request)", model, credits_per_req);
    println!("  Session: {}", session_id);
    println!("  Hostname: {}", hostname);
    println!("  API: {}", api_base);
    println!();
    println!("Registering worker...");

    // Register worker
    let reg_resp = client
        .post(format!("{}/api/v1/workers/register", api_base))
        .json(&serde_json::json!({
            "session_id": session_id,
            "model": model,
            "hostname": hostname,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        }))
        .send()
        .await?;

    let reg: serde_json::Value = reg_resp.json().await?;
    let worker_id = reg["worker_id"].as_str().unwrap_or("unknown");
    println!("  Worker ID: {}", worker_id);
    println!();
    println!("Waiting for inference requests... (Ctrl+C to stop)");
    println!();

    let mut total_earned: u64 = 0;

    loop {
        // Long-poll for work
        match client
            .get(format!("{}/api/v1/workers/poll", api_base))
            .query(&[("worker_id", worker_id), ("model", &model)])
            .send()
            .await
        {
            Ok(resp) => {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                if let Some(request_id) = body["request_id"].as_str() {
                    let prompt = body["prompt"].as_str().unwrap_or("");
                    println!("  Request: {} ({} chars)", request_id, prompt.len());

                    // For now, return a placeholder — actual inference via candle
                    // would go here when local-fallback feature is enabled
                    let result = format!("Worker {} processed request (model: {})", worker_id, model);

                    match client
                        .post(format!("{}/api/v1/workers/result", api_base))
                        .json(&serde_json::json!({
                            "worker_id": worker_id,
                            "request_id": request_id,
                            "result": result,
                        }))
                        .send()
                        .await
                    {
                        Ok(r) => {
                            let d: serde_json::Value = r.json().await.unwrap_or_default();
                            let earned = d["credits_earned"].as_u64().unwrap_or(credits_per_req as u64);
                            total_earned += earned;
                            println!("  +{} credits (total: {})", earned, total_earned);
                        }
                        Err(e) => tracing::warn!("Result submission error: {}", e),
                    }
                }
                // else: no work available, loop again
            }
            Err(e) => {
                tracing::warn!("Poll error: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
}

fn cmd_cron_list(all: bool) -> Result<()> {
    let store_path = config::get_data_dir().join("cron").join("jobs.json");
    let mut service = nanobot_core::service::cron::CronService::new(store_path);
    service.init();

    let jobs = service.list_jobs(all);
    if jobs.is_empty() {
        println!("No scheduled jobs.");
        return Ok(());
    }

    println!("Scheduled Jobs\n");
    println!(
        "  {:<10} {:<20} {:<15} {:<10}",
        "ID", "Name", "Schedule", "Status"
    );
    println!("  {}", "-".repeat(55));

    for job in &jobs {
        let sched = match &job.schedule {
            nanobot_core::service::cron::CronSchedule::Every { every_ms } => {
                format!("every {}s", every_ms / 1000)
            }
            nanobot_core::service::cron::CronSchedule::Cron { expr, .. } => expr.clone(),
            nanobot_core::service::cron::CronSchedule::At { .. } => "one-time".to_string(),
        };
        let status = if job.enabled { "enabled" } else { "disabled" };
        println!(
            "  {:<10} {:<20} {:<15} {:<10}",
            job.id, job.name, sched, status
        );
    }

    Ok(())
}

fn cmd_cron_add(name: String, message: String, every: Option<u64>, cron_expr: Option<String>) -> Result<()> {
    use nanobot_core::service::cron::{CronSchedule, CronService};

    let schedule = if let Some(secs) = every {
        CronSchedule::Every {
            every_ms: secs * 1000,
        }
    } else if let Some(expr) = cron_expr {
        CronSchedule::Cron { expr, tz: None }
    } else {
        eprintln!("Error: Must specify --every or --cron");
        std::process::exit(1);
    };

    let store_path = config::get_data_dir().join("cron").join("jobs.json");
    let mut service = CronService::new(store_path);
    service.init();

    let job = service.add_job(&name, schedule, &message, false, None, None);
    println!("✓ Added job '{}' ({})", job.name, job.id);

    Ok(())
}

fn cmd_cron_remove(job_id: String) -> Result<()> {
    let store_path = config::get_data_dir().join("cron").join("jobs.json");
    let mut service = nanobot_core::service::cron::CronService::new(store_path);
    service.init();

    if service.remove_job(&job_id) {
        println!("✓ Removed job {}", job_id);
    } else {
        println!("Job {} not found", job_id);
    }

    Ok(())
}
