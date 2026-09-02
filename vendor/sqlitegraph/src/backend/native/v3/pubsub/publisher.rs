//! V3 Native Pub/Sub - Publisher
//!
//! This module implements the event delivery mechanism using Rust channels.
//! Events are emitted synchronously on the commit path (no background threads).

use crate::backend::native::v3::pubsub::types::{PubSubEvent, SubscriberId, SubscriptionFilter};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

/// Type alias for subscriber channel entries.
type SubscriberEntry = (SubscriberId, Sender<PubSubEvent>, SubscriptionFilter);

/// Publishes events to subscribers via channels
///
/// The Publisher maintains a list of subscribers with their channels and filters.
/// When an event is emitted, it is delivered to all subscribers whose filters match.
///
/// # Best-Effort Delivery
///
/// This is a **best-effort** system:
/// - If a channel is full, the event is dropped
/// - If a receiver has been dropped, the event is dropped
/// - No blocking, no retries, no guarantees
///
/// This design ensures that slow or dead subscribers cannot block the commit path.
#[derive(Debug)]
pub struct Publisher {
    /// Channel senders for each subscriber
    /// Each tuple contains: (SubscriberId, Sender, SubscriptionFilter)
    senders: Arc<Mutex<Vec<SubscriberEntry>>>,
    /// Next subscriber ID (lock-free atomic counter)
    next_id: AtomicU64,
}

impl Default for Publisher {
    fn default() -> Self {
        Self::new()
    }
}

impl Publisher {
    /// Create a new publisher
    pub fn new() -> Self {
        Self {
            senders: Arc::new(Mutex::new(Vec::new())),
            next_id: AtomicU64::new(1),
        }
    }

    /// Subscribe to events with a filter
    ///
    /// Creates a new channel for this subscriber and returns the receiver end.
    /// The subscriber will receive only events that match the provided filter.
    ///
    /// # Arguments
    /// * `filter` - Filter for which events this subscriber receives
    ///
    /// # Returns
    /// A tuple of (SubscriberId, Receiver<PubSubEvent>)
    pub fn subscribe(&self, filter: SubscriptionFilter) -> (SubscriberId, Receiver<PubSubEvent>) {
        let (tx, rx) = mpsc::channel();

        let id = SubscriberId::new(self.next_id.fetch_add(1, Ordering::Relaxed));

        let mut senders = self.senders.lock();

        senders.push((id, tx, filter));

        (id, rx)
    }

    /// Unsubscribe from events
    ///
    /// Cancels the subscription and stops receiving events.
    /// Returns true if subscription existed and was removed.
    ///
    /// # Arguments
    /// * `subscriber_id` - The subscriber ID returned by subscribe()
    pub fn unsubscribe(&self, subscriber_id: SubscriberId) -> bool {
        let mut senders = self.senders.lock();
        let pos = senders.iter().position(|(id, _, _)| *id == subscriber_id);

        if let Some(pos) = pos {
            senders.swap_remove(pos);
            true
        } else {
            false
        }
    }

    /// Emit an event to all matching subscribers
    ///
    /// This is **synchronous** and **best-effort**:
    /// - Events are dropped if the channel is full
    /// - Events are dropped if the receiver has been dropped
    /// - No blocking or retries
    ///
    /// # Arguments
    /// * `event` - The event to emit
    pub fn emit(&self, event: PubSubEvent) {
        let senders = self.senders.lock();

        for (_, sender, filter) in senders.iter() {
            if filter.matches(&event) {
                // Best-effort delivery - ignore errors
                let _ = sender.send(event.clone());
            }
        }
    }

    /// Get the number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        let senders = self.senders.lock();
        senders.len()
    }

    /// Check if there are any subscribers
    pub fn has_subscribers(&self) -> bool {
        self.subscriber_count() > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_publisher_creation() {
        let publisher = Publisher::new();
        assert_eq!(publisher.subscriber_count(), 0);
        assert!(!publisher.has_subscribers());
    }

    #[test]
    fn test_subscribe_unsubscribe() {
        let publisher = Publisher::new();

        let (id, _rx) = publisher.subscribe(SubscriptionFilter::all());
        assert_eq!(publisher.subscriber_count(), 1);
        assert!(publisher.has_subscribers());

        let removed = publisher.unsubscribe(id);
        assert!(removed);
        assert_eq!(publisher.subscriber_count(), 0);

        // Unsubscribing again should return false
        let removed = publisher.unsubscribe(id);
        assert!(!removed);
    }

    #[test]
    fn test_emit_event() {
        let publisher = Publisher::new();

        let (id, rx) = publisher.subscribe(SubscriptionFilter::all());

        let event = PubSubEvent::NodeChanged {
            node_id: 1,
            snapshot_id: 1,
        };
        publisher.emit(event.clone());

        // Should receive the event
        let received = rx.recv_timeout(Duration::from_millis(100));
        assert_eq!(received, Ok(event));

        publisher.unsubscribe(id);
    }

    #[test]
    fn test_filter_matching() {
        let publisher = Publisher::new();

        // Subscriber 1: all events
        let (_id1, rx1) = publisher.subscribe(SubscriptionFilter::all());

        // Subscriber 2: nodes only
        let (_id2, rx2) = publisher.subscribe(SubscriptionFilter::nodes_only());

        // Emit node change - both should receive
        let node_event = PubSubEvent::NodeChanged {
            node_id: 1,
            snapshot_id: 1,
        };
        publisher.emit(node_event.clone());

        assert_eq!(
            rx1.recv_timeout(Duration::from_millis(100)),
            Ok(node_event.clone())
        );
        assert_eq!(rx2.recv_timeout(Duration::from_millis(100)), Ok(node_event));

        // Emit edge change - only subscriber 1 should receive
        let edge_event = PubSubEvent::EdgeChanged {
            edge_id: 1,
            from_node: 1,
            to_node: 2,
            snapshot_id: 1,
        };
        publisher.emit(edge_event.clone());

        assert_eq!(rx1.recv_timeout(Duration::from_millis(100)), Ok(edge_event));
        // rx2 should timeout since it doesn't subscribe to edge changes
        assert!(rx2.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    fn test_multiple_subscribers_receive_independent_events() {
        let publisher = Publisher::new();

        let (_id1, rx1) = publisher.subscribe(SubscriptionFilter::all());
        let (_id2, rx2) = publisher.subscribe(SubscriptionFilter::all());

        let event = PubSubEvent::SnapshotCommitted { snapshot_id: 42 };
        publisher.emit(event.clone());

        // Both should receive the same event
        assert_eq!(
            rx1.recv_timeout(Duration::from_millis(100)),
            Ok(event.clone())
        );
        assert_eq!(rx2.recv_timeout(Duration::from_millis(100)), Ok(event));
    }
}
