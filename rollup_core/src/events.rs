use async_channel::{Sender, Receiver, unbounded};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use dashmap::DashMap;
use uuid::Uuid;

use crate::hash_utils::Hash;

/// Event types in the rollup
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RollupEvent {
    TransactionSubmitted {
        tx_hash: String,
        timestamp: u64,
    },
    TransactionProcessed {
        tx_hash: String,
        success: bool,
        gas_used: u64,
        timestamp: u64,
    },
    BatchCreated {
        batch_id: u64,
        transaction_count: usize,
        state_root: String,
        timestamp: u64,
    },
    BatchSettled {
        batch_id: u64,
        settlement_tx: String,
        timestamp: u64,
    },
    StateUpdated {
        old_root: String,
        new_root: String,
        timestamp: u64,
    },
    MempoolFull {
        size: usize,
        timestamp: u64,
    },
    FeeUpdated {
        old_base_fee: u64,
        new_base_fee: u64,
        timestamp: u64,
    },
    Error {
        message: String,
        timestamp: u64,
    },
}

impl RollupEvent {
    pub fn timestamp(&self) -> u64 {
        match self {
            RollupEvent::TransactionSubmitted { timestamp, .. } => *timestamp,
            RollupEvent::TransactionProcessed { timestamp, .. } => *timestamp,
            RollupEvent::BatchCreated { timestamp, .. } => *timestamp,
            RollupEvent::BatchSettled { timestamp, .. } => *timestamp,
            RollupEvent::StateUpdated { timestamp, .. } => *timestamp,
            RollupEvent::MempoolFull { timestamp, .. } => *timestamp,
            RollupEvent::FeeUpdated { timestamp, .. } => *timestamp,
            RollupEvent::Error { timestamp, .. } => *timestamp,
        }
    }

    pub fn event_type(&self) -> &str {
        match self {
            RollupEvent::TransactionSubmitted { .. } => "transaction_submitted",
            RollupEvent::TransactionProcessed { .. } => "transaction_processed",
            RollupEvent::BatchCreated { .. } => "batch_created",
            RollupEvent::BatchSettled { .. } => "batch_settled",
            RollupEvent::StateUpdated { .. } => "state_updated",
            RollupEvent::MempoolFull { .. } => "mempool_full",
            RollupEvent::FeeUpdated { .. } => "fee_updated",
            RollupEvent::Error { .. } => "error",
        }
    }
}

/// Subscription to rollup events
pub struct EventSubscription {
    pub id: Uuid,
    pub receiver: Receiver<RollupEvent>,
    pub filter: Option<EventFilter>,
}

/// Filter for events
#[derive(Debug, Clone)]
pub enum EventFilter {
    TransactionEvents,
    BatchEvents,
    StateEvents,
    FeeEvents,
    AllEvents,
}

impl EventFilter {
    pub fn matches(&self, event: &RollupEvent) -> bool {
        match self {
            EventFilter::AllEvents => true,
            EventFilter::TransactionEvents => matches!(
                event,
                RollupEvent::TransactionSubmitted { .. } | RollupEvent::TransactionProcessed { .. }
            ),
            EventFilter::BatchEvents => matches!(
                event,
                RollupEvent::BatchCreated { .. } | RollupEvent::BatchSettled { .. }
            ),
            EventFilter::StateEvents => matches!(event, RollupEvent::StateUpdated { .. }),
            EventFilter::FeeEvents => matches!(event, RollupEvent::FeeUpdated { .. }),
        }
    }
}

/// Event bus for publishing and subscribing to rollup events
pub struct EventBus {
    subscribers: Arc<DashMap<Uuid, (Sender<RollupEvent>, Option<EventFilter>)>>,
    event_history: Arc<parking_lot::RwLock<Vec<RollupEvent>>>,
    max_history_size: usize,
}

impl EventBus {
    pub fn new(max_history_size: usize) -> Self {
        Self {
            subscribers: Arc::new(DashMap::new()),
            event_history: Arc::new(parking_lot::RwLock::new(Vec::new())),
            max_history_size,
        }
    }

    /// Publish an event to all subscribers
    pub fn publish(&self, event: RollupEvent) {
        // Store in history
        {
            let mut history = self.event_history.write();
            history.push(event.clone());

            // Trim history if too large
            if history.len() > self.max_history_size {
                history.drain(0..history.len() - self.max_history_size);
            }
        }

        // Send to all subscribers
        let event_type = event.event_type();
        let mut removed_subscribers = Vec::new();

        for entry in self.subscribers.iter() {
            let (sender, filter) = entry.value();

            // Check if event matches filter
            if let Some(filter) = filter {
                if !filter.matches(&event) {
                    continue;
                }
            }

            // Try to send event
            if sender.try_send(event.clone()).is_err() {
                // Subscriber is closed or full, remove it
                removed_subscribers.push(*entry.key());
            }
        }

        // Remove closed subscribers
        for id in removed_subscribers {
            self.subscribers.remove(&id);
        }

        log::debug!("Published event: {}", event_type);
    }

    /// Subscribe to events
    pub fn subscribe(&self, filter: Option<EventFilter>) -> EventSubscription {
        let (sender, receiver) = unbounded();
        let id = Uuid::new_v4();

        self.subscribers.insert(id, (sender, filter.clone()));

        log::info!("New event subscription: {}", id);

        EventSubscription {
            id,
            receiver,
            filter,
        }
    }

    /// Unsubscribe from events
    pub fn unsubscribe(&self, id: Uuid) {
        self.subscribers.remove(&id);
        log::info!("Removed event subscription: {}", id);
    }

    /// Get event history
    pub fn get_history(&self, limit: Option<usize>) -> Vec<RollupEvent> {
        let history = self.event_history.read();

        if let Some(limit) = limit {
            let start = history.len().saturating_sub(limit);
            history[start..].to_vec()
        } else {
            history.clone()
        }
    }

    /// Get event history filtered by type
    pub fn get_history_filtered(&self, filter: EventFilter, limit: Option<usize>) -> Vec<RollupEvent> {
        let history = self.event_history.read();
        let filtered: Vec<_> = history
            .iter()
            .filter(|event| filter.matches(event))
            .cloned()
            .collect();

        if let Some(limit) = limit {
            let start = filtered.len().saturating_sub(limit);
            filtered[start..].to_vec()
        } else {
            filtered
        }
    }

    /// Get subscriber count
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Clear event history
    pub fn clear_history(&self) {
        self.event_history.write().clear();
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1000) // Keep last 1000 events by default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_publish_subscribe() {
        let bus = EventBus::new(100);

        let subscription = bus.subscribe(Some(EventFilter::TransactionEvents));

        let event = RollupEvent::TransactionSubmitted {
            tx_hash: "test".to_string(),
            timestamp: 12345,
        };

        bus.publish(event);

        // Should receive the event
        let received = subscription.receiver.try_recv().unwrap();
        assert_eq!(received.event_type(), "transaction_submitted");
    }

    #[test]
    fn test_event_filtering() {
        let bus = EventBus::new(100);

        let subscription = bus.subscribe(Some(EventFilter::TransactionEvents));

        // Publish a batch event (should not be received)
        bus.publish(RollupEvent::BatchCreated {
            batch_id: 1,
            transaction_count: 10,
            state_root: "root".to_string(),
            timestamp: 12345,
        });

        // Should not receive the event
        assert!(subscription.receiver.try_recv().is_err());

        // Publish a transaction event (should be received)
        bus.publish(RollupEvent::TransactionSubmitted {
            tx_hash: "test".to_string(),
            timestamp: 12345,
        });

        // Should receive this event
        assert!(subscription.receiver.try_recv().is_ok());
    }

    #[test]
    fn test_event_history() {
        let bus = EventBus::new(100);

        for i in 0..10 {
            bus.publish(RollupEvent::TransactionSubmitted {
                tx_hash: format!("tx{}", i),
                timestamp: i,
            });
        }

        let history = bus.get_history(Some(5));
        assert_eq!(history.len(), 5);
    }

    #[test]
    fn test_history_limit() {
        let bus = EventBus::new(5); // Max 5 events

        for i in 0..10 {
            bus.publish(RollupEvent::TransactionSubmitted {
                tx_hash: format!("tx{}", i),
                timestamp: i,
            });
        }

        let history = bus.get_history(None);
        assert_eq!(history.len(), 5); // Should only keep last 5
    }
}
