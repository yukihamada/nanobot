use tokio::sync::mpsc;

use crate::types::{InboundMessage, OutboundMessage};

/// Async message bus that decouples chat channels from the agent core.
///
/// Channels push messages to the inbound queue, and the agent processes
/// them and pushes responses to the outbound queue.
pub struct MessageBus {
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: mpsc::Receiver<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    outbound_rx: mpsc::Receiver<OutboundMessage>,
}

impl MessageBus {
    pub fn new(buffer_size: usize) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(buffer_size);
        let (outbound_tx, outbound_rx) = mpsc::channel(buffer_size);
        Self {
            inbound_tx,
            inbound_rx,
            outbound_tx,
            outbound_rx,
        }
    }

    /// Get a sender handle for publishing inbound messages (from channels).
    pub fn inbound_sender(&self) -> mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    /// Get a sender handle for publishing outbound messages (from agent).
    pub fn outbound_sender(&self) -> mpsc::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }

    /// Consume the next inbound message (blocks until available).
    pub async fn consume_inbound(&mut self) -> Option<InboundMessage> {
        self.inbound_rx.recv().await
    }

    /// Consume the next outbound message (blocks until available).
    pub async fn consume_outbound(&mut self) -> Option<OutboundMessage> {
        self.outbound_rx.recv().await
    }

    /// Publish an inbound message.
    pub async fn publish_inbound(&self, msg: InboundMessage) -> Result<(), mpsc::error::SendError<InboundMessage>> {
        self.inbound_tx.send(msg).await
    }

    /// Publish an outbound message.
    pub async fn publish_outbound(&self, msg: OutboundMessage) -> Result<(), mpsc::error::SendError<OutboundMessage>> {
        self.outbound_tx.send(msg).await
    }

    /// Split the bus into inbound/outbound halves for concurrent use.
    pub fn split(self) -> (InboundBus, OutboundBus) {
        (
            InboundBus {
                tx: self.inbound_tx,
                rx: self.inbound_rx,
            },
            OutboundBus {
                tx: self.outbound_tx,
                rx: self.outbound_rx,
            },
        )
    }
}

/// Inbound half of the message bus.
pub struct InboundBus {
    pub tx: mpsc::Sender<InboundMessage>,
    pub rx: mpsc::Receiver<InboundMessage>,
}

/// Outbound half of the message bus.
pub struct OutboundBus {
    pub tx: mpsc::Sender<OutboundMessage>,
    pub rx: mpsc::Receiver<OutboundMessage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bus_inbound_roundtrip() {
        let mut bus = MessageBus::new(16);
        let tx = bus.inbound_sender();

        let msg = InboundMessage::new("test", "user1", "chat1", "hello");
        tx.send(msg).await.unwrap();

        let received = bus.consume_inbound().await.unwrap();
        assert_eq!(received.content, "hello");
        assert_eq!(received.channel, "test");
    }

    #[tokio::test]
    async fn test_bus_outbound_roundtrip() {
        let mut bus = MessageBus::new(16);
        let tx = bus.outbound_sender();

        let msg = OutboundMessage::new("discord", "chan1", "response");
        tx.send(msg).await.unwrap();

        let received = bus.consume_outbound().await.unwrap();
        assert_eq!(received.content, "response");
    }

    #[tokio::test]
    async fn test_bus_publish() {
        let mut bus = MessageBus::new(16);

        let msg = InboundMessage::new("test", "u", "c", "via publish");
        bus.publish_inbound(msg).await.unwrap();

        let received = bus.consume_inbound().await.unwrap();
        assert_eq!(received.content, "via publish");
    }

    #[tokio::test]
    async fn test_bus_split() {
        let bus = MessageBus::new(16);
        let (mut inbound, mut outbound) = bus.split();

        let msg_in = InboundMessage::new("ch", "u", "c", "in");
        inbound.tx.send(msg_in).await.unwrap();
        let received = inbound.rx.recv().await.unwrap();
        assert_eq!(received.content, "in");

        let msg_out = OutboundMessage::new("ch", "c", "out");
        outbound.tx.send(msg_out).await.unwrap();
        let received = outbound.rx.recv().await.unwrap();
        assert_eq!(received.content, "out");
    }
}
