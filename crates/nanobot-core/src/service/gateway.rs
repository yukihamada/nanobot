use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

use crate::agent::subagent::SubagentManager;
use crate::agent::AgentLoop;
use crate::bus::MessageBus;
use crate::channel::telegram::TelegramChannel;
use crate::channel::discord::DiscordChannel;
use crate::channel::whatsapp::WhatsAppChannel;
use crate::channel::feishu::FeishuChannel;
use crate::channel::line::LineChannel;
use crate::channel::Channel;
use crate::config::Config;
use crate::provider;
use crate::service::cron::CronService;
use crate::service::heartbeat;
use crate::types::{InboundMessage, OutboundMessage};

/// Start the full nanobot gateway with all components.
pub async fn run_gateway(config: Config) -> anyhow::Result<()> {
    let workspace = config.workspace_path();
    std::fs::create_dir_all(&workspace)?;

    // Create message bus
    let bus = MessageBus::new(256);
    let inbound_tx = bus.inbound_sender();
    let _outbound_tx = bus.outbound_sender();

    // Create provider
    let model = config.agents.defaults.model.clone();
    let is_bedrock = model.starts_with("bedrock/");
    let api_key = config.get_api_key(None);

    if api_key.is_none() && !is_bedrock {
        return Err(anyhow::anyhow!(
            "No API key configured. Set one in ~/.nanobot/config.json"
        ));
    }

    let api_key_str = api_key.unwrap_or("").to_string();
    let api_base = config.get_api_base(None).map(|s| s.to_string());
    let llm_provider: Arc<dyn provider::LlmProvider> = Arc::from(provider::create_provider(
        &api_key_str,
        api_base.as_deref(),
        &model,
    ));

    // Create cron service
    let cron_store_path = crate::config::get_data_dir().join("cron").join("jobs.json");
    let mut cron_service = CronService::new(cron_store_path);
    cron_service.init();
    let cron_service = Arc::new(Mutex::new(cron_service));

    // Create subagent manager
    let subagent_manager = Arc::new(SubagentManager::new(
        llm_provider.clone(),
        workspace.clone(),
        model.clone(),
        if config.tools.web.search.api_key.is_empty() {
            None
        } else {
            Some(config.tools.web.search.api_key.clone())
        },
        config.tools.exec_config.clone(),
        config.tools.restrict_to_workspace,
        inbound_tx.clone(),
    ));

    // Create agent
    let brave_api_key = if config.tools.web.search.api_key.is_empty() {
        None
    } else {
        Some(config.tools.web.search.api_key.clone())
    };

    let agent = AgentLoop::new(
        bus,
        llm_provider.clone(),
        workspace.clone(),
        Some(model.clone()),
        config.agents.defaults.max_tool_iterations,
        brave_api_key,
        config.tools.exec_config.clone(),
        config.tools.restrict_to_workspace,
        Some(subagent_manager),
    );

    // Create channels
    let (_channel_inbound_tx, _channel_inbound_rx) = mpsc::channel::<InboundMessage>(256);
    let (_agent_outbound_tx, _agent_outbound_rx) = mpsc::channel::<OutboundMessage>(256);

    let mut channels: Vec<Box<dyn Channel>> = Vec::new();

    if config.channels.telegram.enabled {
        info!("Telegram channel enabled");
        channels.push(Box::new(TelegramChannel::new(
            config.channels.telegram.clone(),
            inbound_tx.clone(),
        )));
    }

    if config.channels.discord.enabled {
        info!("Discord channel enabled");
        channels.push(Box::new(DiscordChannel::new(
            config.channels.discord.clone(),
            inbound_tx.clone(),
        )));
    }

    if config.channels.whatsapp.enabled {
        info!("WhatsApp channel enabled");
        channels.push(Box::new(WhatsAppChannel::new(
            config.channels.whatsapp.clone(),
            inbound_tx.clone(),
        )));
    }

    if config.channels.feishu.enabled {
        info!("Feishu channel enabled");
        channels.push(Box::new(FeishuChannel::new(
            config.channels.feishu.clone(),
            inbound_tx.clone(),
        )));
    }

    if config.channels.line.enabled {
        info!("LINE channel enabled");
        channels.push(Box::new(LineChannel::new(
            config.channels.line.clone(),
            inbound_tx.clone(),
        )));
    }

    let enabled_names: Vec<&str> = channels.iter().map(|c| c.name()).collect();
    if !enabled_names.is_empty() {
        info!("Channels enabled: {}", enabled_names.join(", "));
    } else {
        warn!("No channels enabled");
    }

    // Start heartbeat in background
    let hb_workspace = workspace.clone();
    let hb_interval = 30 * 60; // 30 minutes
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(hb_interval)).await;
            if heartbeat::should_trigger(&hb_workspace) {
                info!("Heartbeat: checking for tasks...");
                // Would trigger agent.process_direct here
            } else {
                tracing::debug!("Heartbeat: no tasks");
            }
        }
    });

    // Start cron scheduler in background
    let cron_clone = cron_service.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let due_jobs = {
                let mut cron = cron_clone.lock().await;
                cron.get_due_jobs()
            };
            for job in due_jobs {
                info!("Cron: executing job '{}' ({})", job.name, job.id);
                // Would trigger agent.process_direct here
                let mut cron = cron_clone.lock().await;
                cron.mark_executed(&job.id, "ok", None);
            }
        }
    });

    info!("nanobot gateway started");

    // Run agent loop (this is the main blocking call)
    // In a full implementation, we'd split inbound/outbound and run concurrently
    // For now, we create the necessary channels and run the agent
    let (_agent_inbound_tx, agent_inbound_rx) = mpsc::channel::<InboundMessage>(256);

    // Start channel tasks
    for mut channel in channels {
        tokio::spawn(async move {
            if let Err(e) = channel.start().await {
                error!("Channel {} error: {}", channel.name(), e);
            }
        });
    }

    // Run agent (blocks)
    agent.run(agent_inbound_rx).await;

    Ok(())
}
