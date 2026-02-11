use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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
        Commands::Chat { message, api, sync } => cmd_chat(message, api, sync).await?,
        Commands::Link { session_id } => cmd_link(session_id).await?,
        Commands::Onboard => cmd_onboard()?,
        Commands::Agent { message, session } => cmd_agent(message, session).await?,
        Commands::Gateway { port, verbose, http, http_port } => cmd_gateway(port, verbose, http, http_port).await?,
        Commands::Daemon { interval, api } => cmd_daemon(interval, api).await?,
        Commands::Status => cmd_status()?,
        Commands::Channels { command } => match command {
            ChannelCommands::Status => cmd_channels_status()?,
        },
        Commands::Cron { command } => match command {
            CronCommands::List { all } => cmd_cron_list(all)?,
            CronCommands::Add {
                name,
                message,
                every,
                cron,
            } => cmd_cron_add(name, message, every, cron)?,
            CronCommands::Remove { job_id } => cmd_cron_remove(job_id)?,
        },
        Commands::Earn { model, api } => cmd_earn(model, api).await?,
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

/// Chat with chatweb.ai API directly — no config or API key needed.
async fn cmd_chat(message: Vec<String>, api_url: String, sync: Option<String>) -> Result<()> {
    let session_id = if let Some(ref sid) = sync {
        // Use the provided session ID directly (sync with Web/LINE/Telegram)
        sid.clone()
    } else {
        get_cli_session_id()?
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .unwrap_or_default();

    if message.is_empty() {
        // Interactive mode
        println!("{} chatweb.ai CLI (Ctrl+C to exit)", nanobot_core::LOGO);
        println!("  Session: {}", session_id);
        if sync.is_some() {
            println!("  Synced with Web session");
        }
        println!();

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

            match chat_api(&client, &api_url, input, &session_id).await {
                Ok(resp) => println!("\n{} {}\n", nanobot_core::LOGO, resp),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    } else {
        // Single message mode
        let msg = message.join(" ");
        match chat_api(&client, &api_url, &msg, &session_id).await {
            Ok(resp) => println!("{}", resp),
            Err(e) => eprintln!("Error: {}", e),
        }
    }

    Ok(())
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

async fn chat_api(client: &reqwest::Client, api_url: &str, message: &str, session_id: &str) -> Result<String> {
    let resp = match client
        .post(api_url)
        .json(&serde_json::json!({
            "message": message,
            "session_id": session_id,
        }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_timeout() => {
            return Ok("ごめんね、ちょっと考えすぎちゃった...もう一回聞いてくれる？".to_string());
        }
        Err(e) => return Err(e.into()),
    };

    let body: serde_json::Value = resp.json().await?;
    Ok(body["response"].as_str().unwrap_or("No response").to_string())
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
async fn cmd_gateway(port: u16, verbose: bool, http: bool, http_port: u16) -> Result<()> {
    if verbose {
        // Re-init with debug level
        // Already handled by env filter
    }

    let cfg = config::load_config_from_env();

    #[cfg(feature = "http-api")]
    if http {
        use nanobot_core::service::http::{serve, AppState};
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

        // Run HTTP server and gateway concurrently
        let http_handle = tokio::spawn(async move {
            if let Err(e) = serve(&addr, state).await {
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
