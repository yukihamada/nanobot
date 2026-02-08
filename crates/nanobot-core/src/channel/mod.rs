pub mod telegram;
pub mod discord;
pub mod whatsapp;
pub mod feishu;
pub mod line;
pub mod slack;
pub mod signal;
pub mod imessage;
pub mod teams;
pub mod google_chat;
pub mod matrix;
pub mod zalo;
pub mod facebook;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::types::OutboundMessage;

/// Trait for chat channel implementations.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Channel name (e.g., "telegram", "discord").
    fn name(&self) -> &str;

    /// Start the channel and begin listening for messages.
    async fn start(&mut self) -> anyhow::Result<()>;

    /// Stop the channel.
    async fn stop(&mut self) -> anyhow::Result<()>;

    /// Send a message through this channel.
    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()>;

    /// Check if the channel is running.
    fn is_running(&self) -> bool;
}

/// Check if a sender is allowed based on the allow list.
pub fn is_allowed(sender_id: &str, allow_from: &[String]) -> bool {
    if allow_from.is_empty() {
        return true;
    }
    if allow_from.contains(&sender_id.to_string()) {
        return true;
    }
    // Check pipe-separated IDs (e.g., "123456|username")
    if sender_id.contains('|') {
        for part in sender_id.split('|') {
            if !part.is_empty() && allow_from.contains(&part.to_string()) {
                return true;
            }
        }
    }
    false
}

/// Channel manager that coordinates multiple channels.
pub struct ChannelManager {
    channels: Vec<Box<dyn Channel>>,
    outbound_rx: Option<mpsc::Receiver<OutboundMessage>>,
}

impl ChannelManager {
    pub fn new(outbound_rx: mpsc::Receiver<OutboundMessage>) -> Self {
        Self {
            channels: Vec::new(),
            outbound_rx: Some(outbound_rx),
        }
    }

    /// Add a channel.
    pub fn add_channel(&mut self, channel: Box<dyn Channel>) {
        self.channels.push(channel);
    }

    /// Get list of enabled channel names.
    pub fn enabled_channels(&self) -> Vec<&str> {
        self.channels.iter().map(|c| c.name()).collect()
    }

    /// Start all channels and the outbound dispatcher.
    pub async fn start_all(&mut self) -> anyhow::Result<()> {
        if self.channels.is_empty() {
            tracing::warn!("No channels enabled");
            return Ok(());
        }

        // Start all channels concurrently
        let _handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

        // We need to move channels out for the async tasks
        // For simplicity, start channels sequentially and dispatch in background
        for channel in &mut self.channels {
            tracing::info!("Starting {} channel...", channel.name());
            if let Err(e) = channel.start().await {
                tracing::error!("Failed to start {} channel: {}", channel.name(), e);
            }
        }

        Ok(())
    }

    /// Dispatch outbound messages to the appropriate channel.
    pub async fn dispatch_outbound(&mut self) {
        let mut rx = match self.outbound_rx.take() {
            Some(rx) => rx,
            None => return,
        };

        while let Some(msg) = rx.recv().await {
            let mut sent = false;
            for channel in &self.channels {
                if channel.name() == msg.channel {
                    if let Err(e) = channel.send(&msg).await {
                        tracing::error!("Error sending to {}: {}", msg.channel, e);
                    }
                    sent = true;
                    break;
                }
            }
            if !sent {
                tracing::warn!("Unknown channel: {}", msg.channel);
            }
        }
    }

    /// Stop all channels.
    pub async fn stop_all(&mut self) {
        for channel in &mut self.channels {
            if let Err(e) = channel.stop().await {
                tracing::error!("Error stopping {}: {}", channel.name(), e);
            }
        }
    }
}
