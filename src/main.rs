use std::sync::Arc;
use std::collections::HashMap;

use anyhow::Result;

// Use mimalloc for better performance (disabled for Lambda compatibility testing)
// #[global_allocator]
// static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
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
    /// Update the chatweb CLI to the latest version
    Update,
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
        /// Cron schedule (e.g. "0 0 * * *")
        #[arg(short, long)]
        schedule: String,
        /// Session ID
        #[arg(short, long, default_value = "cli:default")]
        session: String,
        /// Enable the job immediately
        #[arg(short, long)]
        enable: bool,
    },
    /// Remove a scheduled job
    Remove {
        /// Job name
        #[arg(short, long)]
        name: String,
    },
    /// Enable a scheduled job
    Enable {
        /// Job name
        #[arg(short, long)]
        name: String,
    },
    /// Disable a scheduled job
    Disable {
        /// Job name
        #[arg(short, long)]
        name: String,
    },
    /// Run a scheduled job immediately
    Run {
        /// Job name
        #[arg(short, long)]
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = Arc::new(Config::load()?);
    let bus = Arc::new(MessageBus::new(config.clone()));

    match cli.command {
        Some(Commands::Voice { api, sync }) => {
            nanobot_core::voice::run(bus.clone(), &api, sync.as_deref()).await?;
        }
        Some(Commands::Chat { message, api, sync }) => {
            let message = if message.is_empty() {
                // Interactive mode
                nanobot_core::chat::run_interactive(bus.clone(), &api, sync.as_deref()).await?
            } else {
                // One-shot message
                nanobot_core::chat::run_oneshot(bus.clone(), &message.join(" "), &api, sync.as_deref()).await?
            };
            println!("{}", message);
        }
        Some(Commands::Link { session_id }) => {
            nanobot_core::link::run(bus.clone(), session_id.as_deref()).await?;
        }
        Some(Commands::Onboard) => {
            nanobot_core::onboard::run(bus.clone()).await?;
        }
        Some(Commands::Agent { message, session }) => {
            nanobot_core::agent::run(bus.clone(), message.as_deref(), &session).await?;
        }
        Some(Commands::Gateway {
            port,
            verbose,
            http,
            http_port,
            auth,
        }) => {
            nanobot_core::gateway::run(bus.clone(), port, verbose, http, http_port, auth).await?;
        }
        Some(Commands::Status) => {
            nanobot_core::status::run(bus.clone()).await?;
        }
        Some(Commands::Channels { command }) => match command {
            ChannelCommands::Status => {
                nanobot_core::channel::status(bus.clone()).await?;
            }
        },
        Some(Commands::Daemon { interval, api }) => {
            nanobot_core::daemon::run(bus.clone(), interval, &api).await?;
        }
        Some(Commands::Cron { command }) => match command {
            CronCommands::List { all } => {
                nanobot_core::cron::list(bus.clone(), all).await?;
            }
            CronCommands::Add {
                name,
                message,
                schedule,
                session,
                enable,
            } => {
                nanobot_core::cron::add(bus.clone(), &name, &message, &schedule, &session, enable)
                    .await?;
            }
            CronCommands::Remove { name } => {
                nanobot_core::cron::remove(bus.clone(), &name).await?;
            }
            CronCommands::Enable { name } => {
                nanobot_core::cron::enable(bus.clone(), &name).await?;
            }
            CronCommands::Disable { name } => {
                nanobot_core::cron::disable(bus.clone(), &name).await?;
            }
            CronCommands::Run { name } => {
                nanobot_core::cron::run_job(bus.clone(), &name).await?;
            }
        },
        Some(Commands::Earn { model, api }) => {
            nanobot_core::earn::run(bus.clone(), &model, &api).await?;
        }
        Some(Commands::GenToken) => {
            nanobot_core::gen_token::run(bus.clone()).await?;
        }
        Some(Commands::Update) => {
            // TODO: Implement update logic
            println!("Checking for updates...");
            println!("Update command is not yet implemented.");
        }
        None => {
            // Default command (Voice)
            nanobot_core::voice::run(bus.clone(), "https://chatweb.ai/api/v1/chat", None).await?;
        }
    }

    Ok(())
}
