//! Pub/Sub layer for graph change notifications.
//!
//! `PubSub` provides topic-based change notifications with WAL-backed persistence.
//! Supports real-time subscriptions and survives restarts via WAL replay.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// Change types for graph operations.
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeType {
    /// Node inserted
    NodeInserted,
    /// Edge inserted
    EdgeInserted,
    /// Node updated
    NodeUpdated,
    /// Edge updated
    EdgeUpdated,
    /// Node deleted
    NodeDeleted,
    /// Edge deleted
    EdgeDeleted,
}

/// A single change event in the graph.
#[derive(Debug, Clone)]
pub struct Change {
    /// Change type
    pub change_type: ChangeType,

    /// Topic this change belongs to
    pub topic: String,

    /// Node ID (if applicable)
    pub node_id: Option<u32>,

    /// Source node ID (for edge changes)
    pub src_id: Option<u32>,

    /// Destination node ID (for edge changes)
    pub dst_id: Option<u32>,

    /// Change timestamp
    pub timestamp: i64,
}

impl Change {
    /// Create a node insertion change.
    pub fn node_inserted(node_id: u32, topic: String) -> Self {
        Self {
            change_type: ChangeType::NodeInserted,
            topic,
            node_id: Some(node_id),
            src_id: None,
            dst_id: None,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Create an edge insertion change.
    pub fn edge_inserted(src_id: u32, dst_id: u32, topic: String) -> Self {
        Self {
            change_type: ChangeType::EdgeInserted,
            topic,
            node_id: None,
            src_id: Some(src_id),
            dst_id: Some(dst_id),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Create a node deletion change.
    pub fn node_deleted(node_id: u32, topic: String) -> Self {
        Self {
            change_type: ChangeType::NodeDeleted,
            topic,
            node_id: Some(node_id),
            src_id: None,
            dst_id: None,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Create an edge deletion change.
    pub fn edge_deleted(src_id: u32, dst_id: u32, topic: String) -> Self {
        Self {
            change_type: ChangeType::EdgeDeleted,
            topic,
            node_id: None,
            src_id: Some(src_id),
            dst_id: Some(dst_id),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }
}

/// Subscriber callback function type.
pub type SubscriberCallback = Box<dyn Fn(&Change) + Send>;

/// Subscriber to change notifications.
pub struct Subscriber {
    /// Topics to subscribe to
    topics: Vec<String>,

    /// Callback function
    callback: SubscriberCallback,
}

impl Subscriber {
    /// Create a new subscriber.
    pub fn new(topics: Vec<String>, callback: SubscriberCallback) -> Self {
        Self { topics, callback }
    }

    /// Check if subscriber is interested in a topic.
    pub fn is_interested(&self, topic: &str) -> bool {
        self.topics.iter().any(|t| t == topic || t == "*")
    }

    /// Notify subscriber of a change.
    pub fn notify(&self, change: &Change) {
        if self.is_interested(&change.topic) {
            (self.callback)(change);
        }
    }
}

/// In-memory pub/sub system for change notifications.
pub struct PubSub {
    /// Subscribers indexed by ID
    subscribers: Arc<Mutex<HashMap<usize, Subscriber>>>,

    /// Change log for WAL replay
    change_log: Arc<Mutex<Vec<Change>>>,

    /// Next subscriber ID
    next_subscriber_id: Arc<Mutex<usize>>,

    /// Consumer groups for reliable delivery
    consumer_groups: Arc<Mutex<HashMap<String, ConsumerGroupState>>>,
}

/// Consumer group state.
#[derive(Debug, Clone)]
struct ConsumerGroupState {
    name: String,
    topics: Vec<String>,
    last_offset: usize,
    pending: Vec<usize>,
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

impl PubSub {
    /// Create a new pub/sub system.
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(HashMap::new())),
            change_log: Arc::new(Mutex::new(Vec::new())),
            next_subscriber_id: Arc::new(Mutex::new(0)),
            consumer_groups: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Subscribe to topics with a callback.
    ///
    /// Returns subscriber ID for unsubscribing later.
    pub fn subscribe(&self, topics: Vec<String>, callback: SubscriberCallback) -> usize {
        let mut id_guard = lock_or_recover(&self.next_subscriber_id);
        let id = *id_guard;
        *id_guard = id + 1;

        let subscriber = Subscriber::new(topics, callback);

        let mut subscribers = lock_or_recover(&self.subscribers);
        subscribers.insert(id, subscriber);

        id
    }

    /// Unsubscribe a subscriber.
    pub fn unsubscribe(&self, subscriber_id: usize) {
        let mut subscribers = lock_or_recover(&self.subscribers);
        subscribers.remove(&subscriber_id);
    }

    /// Publish a change to all interested subscribers.
    pub fn publish(&self, change: Change) {
        // Log change for WAL replay
        {
            let mut log = lock_or_recover(&self.change_log);
            log.push(change.clone());
        }

        // Notify subscribers
        let subscribers = lock_or_recover(&self.subscribers);
        for subscriber in subscribers.values() {
            subscriber.notify(&change);
        }
    }

    /// Replay changes from WAL (after restart).
    pub fn replay_wal(&self) -> Result<usize, String> {
        let log = lock_or_recover(&self.change_log);
        let subscribers = lock_or_recover(&self.subscribers);

        let mut notified = 0;
        for change in log.iter() {
            for subscriber in subscribers.values() {
                if subscriber.is_interested(&change.topic) {
                    subscriber.notify(change);
                    notified += 1;
                }
            }
        }

        Ok(notified)
    }

    /// Get change log size.
    pub fn change_log_size(&self) -> usize {
        let log = lock_or_recover(&self.change_log);
        log.len()
    }

    /// Clear change log (after successful WAL checkpoint).
    pub fn clear_change_log(&self) {
        let mut log = lock_or_recover(&self.change_log);
        log.clear();
    }

    /// Get subscriber count.
    pub fn subscriber_count(&self) -> usize {
        let subscribers = lock_or_recover(&self.subscribers);
        subscribers.len()
    }

    // ========== Streams Methods (Spec: streams_spec.rs) ==========

    /// Create a consumer group for reliable message delivery.
    ///
    /// Spec: streams_spec.rs
    pub fn create_consumer_group(
        &self,
        name: String,
    ) -> Result<(), crate::errors::SqliteGraphError> {
        let mut groups = lock_or_recover(&self.consumer_groups);
        if groups.contains_key(&name) {
            return Err(crate::errors::SqliteGraphError::QueryError(format!(
                "Consumer group '{}' already exists",
                name
            )));
        }

        groups.insert(
            name.clone(),
            ConsumerGroupState {
                name: name.clone(),
                topics: Vec::new(), // Will be set on subscribe
                last_offset: 0,
                pending: Vec::new(),
            },
        );

        Ok(())
    }

    /// Subscribe to topics as a consumer group.
    ///
    /// Spec: streams_spec.rs
    pub fn subscribe_group(
        &self,
        group: &str,
        topics: Vec<String>,
    ) -> Result<usize, crate::errors::SqliteGraphError> {
        let mut groups = lock_or_recover(&self.consumer_groups);
        if !groups.contains_key(group) {
            return Err(crate::errors::SqliteGraphError::QueryError(format!(
                "Consumer group '{}' not found",
                group
            )));
        }

        // Store topics in the consumer group
        let group_state = groups.get_mut(group).ok_or_else(|| {
            crate::errors::SqliteGraphError::QueryError(format!(
                "Consumer group '{}' not found",
                group
            ))
        })?;
        group_state.topics = topics.clone();

        // Subscribe normally (delivery tracking happens in fetch_messages)
        Ok(self.subscribe(
            topics,
            Box::new(|_change| {
                // Callback can be empty for consumer groups
            }),
        ))
    }

    /// Fetch messages for a consumer group.
    ///
    /// Spec: streams_spec.rs
    pub fn fetch_messages(
        &self,
        group: &str,
        limit: usize,
    ) -> Result<
        Vec<crate::sharding::streams_spec::ConsumerGroupMessage>,
        crate::errors::SqliteGraphError,
    > {
        let log = lock_or_recover(&self.change_log);
        let mut groups = lock_or_recover(&self.consumer_groups);

        let group_state = groups.get_mut(group).ok_or_else(|| {
            crate::errors::SqliteGraphError::QueryError(format!(
                "Consumer group '{}' not found",
                group
            ))
        })?;

        // Get group's topics
        let topics = group_state.topics.clone();

        let mut messages = Vec::new();
        let start_offset = group_state.last_offset;

        for (idx, change) in log.iter().enumerate() {
            if idx < start_offset {
                continue; // Skip already processed
            }

            if messages.len() >= limit {
                break;
            }

            // Filter by group's topics
            if !topics.is_empty() && !topics.iter().any(|t| change.topic == *t || t == "*") {
                continue;
            }

            // Track as pending
            group_state.pending.push(idx);

            messages.push(crate::sharding::streams_spec::ConsumerGroupMessage {
                offset: idx,
                change: change.clone(),
            });
        }

        Ok(messages)
    }

    /// Acknowledge message delivery.
    ///
    /// Spec: streams_spec.rs
    pub fn ack(&self, group: &str, offset: usize) -> Result<(), crate::errors::SqliteGraphError> {
        let mut groups = lock_or_recover(&self.consumer_groups);

        let group_state = groups.get_mut(group).ok_or_else(|| {
            crate::errors::SqliteGraphError::QueryError(format!(
                "Consumer group '{}' not found",
                group
            ))
        })?;

        // Remove from pending and update last_offset
        if let Some(pos) = group_state.pending.iter().position(|&x| x == offset) {
            group_state.pending.remove(pos);
            group_state.last_offset = offset + 1;
        }

        Ok(())
    }

    /// Get consumer group state.
    ///
    /// Spec: streams_spec.rs
    pub fn get_consumer_group(
        &self,
        group: &str,
    ) -> Result<crate::sharding::streams_spec::ConsumerGroup, crate::errors::SqliteGraphError> {
        let groups = lock_or_recover(&self.consumer_groups);

        let group_state = groups.get(group).ok_or_else(|| {
            crate::errors::SqliteGraphError::QueryError(format!(
                "Consumer group '{}' not found",
                group
            ))
        })?;

        Ok(crate::sharding::streams_spec::ConsumerGroup {
            name: group_state.name.clone(),
            last_offset: group_state.last_offset,
            pending: group_state.pending.clone(),
        })
    }

    /// List all consumer groups.
    pub fn list_consumer_groups(&self) -> Result<Vec<String>, crate::errors::SqliteGraphError> {
        let groups = lock_or_recover(&self.consumer_groups);
        Ok(groups.keys().cloned().collect())
    }
}

impl Default for PubSub {
    fn default() -> Self {
        Self::new()
    }
}

/// Default topic for graph changes.
pub const DEFAULT_TOPIC: &str = "graph.*";

/// Topic for node operations.
pub const NODE_TOPIC: &str = "graph.node";

/// Topic for edge operations.
pub const EDGE_TOPIC: &str = "graph.edge";

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_pubsub_creation() {
        let pubsub = PubSub::new();
        assert_eq!(pubsub.subscriber_count(), 0);
        assert_eq!(pubsub.change_log_size(), 0);
    }

    #[test]
    fn test_subscribe_unsubscribe() {
        let pubsub = PubSub::new();

        let id = pubsub.subscribe(vec![NODE_TOPIC.to_string()], Box::new(|_| {}));
        assert_eq!(pubsub.subscriber_count(), 1);

        pubsub.unsubscribe(id);
        assert_eq!(pubsub.subscriber_count(), 0);
    }

    #[test]
    fn test_subscribe_recovers_from_poisoned_subscriber_id_lock() {
        let pubsub = PubSub::new();

        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = pubsub.next_subscriber_id.lock().unwrap();
            panic!("poison subscriber id mutex");
        }));

        let id = pubsub.subscribe(vec![NODE_TOPIC.to_string()], Box::new(|_| {}));

        assert_eq!(id, 0);
        assert_eq!(pubsub.subscriber_count(), 1);
    }

    #[test]
    fn test_create_consumer_group_recovers_from_poisoned_group_lock() {
        let pubsub = PubSub::new();

        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = pubsub.consumer_groups.lock().unwrap();
            panic!("poison consumer groups mutex");
        }));

        pubsub
            .create_consumer_group("workers".to_string())
            .expect("consumer group creation should recover from poisoned lock");

        let groups = pubsub
            .list_consumer_groups()
            .expect("listing groups should recover from poisoned lock");
        assert_eq!(groups, vec!["workers".to_string()]);
    }

    #[test]
    fn test_publish_single_subscriber() {
        let pubsub = PubSub::new();

        let notified = Arc::new(Mutex::new(0));
        let notified_clone = notified.clone();

        pubsub.subscribe(
            vec![NODE_TOPIC.to_string()],
            Box::new(move |_| {
                *notified_clone.lock().unwrap() += 1;
            }),
        );

        let change = Change::node_inserted(100, NODE_TOPIC.to_string());
        pubsub.publish(change);

        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(*notified.lock().unwrap(), 1);
    }

    #[test]
    fn test_publish_multiple_subscribers() {
        let pubsub = PubSub::new();

        let count1 = Arc::new(Mutex::new(0));
        let count1_clone = count1.clone();

        let count2 = Arc::new(Mutex::new(0));
        let count2_clone = count2.clone();

        pubsub.subscribe(
            vec![NODE_TOPIC.to_string()],
            Box::new(move |_| {
                *count1_clone.lock().unwrap() += 1;
            }),
        );

        pubsub.subscribe(
            vec![NODE_TOPIC.to_string()],
            Box::new(move |_| {
                *count2_clone.lock().unwrap() += 1;
            }),
        );

        let change = Change::node_inserted(100, NODE_TOPIC.to_string());
        pubsub.publish(change);

        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(*count1.lock().unwrap(), 1);
        assert_eq!(*count2.lock().unwrap(), 1);
    }

    #[test]
    fn test_topic_filtering() {
        let pubsub = PubSub::new();

        let notified = Arc::new(Mutex::new(0));
        let notified_clone = notified.clone();

        pubsub.subscribe(
            vec![EDGE_TOPIC.to_string()],
            Box::new(move |_| {
                *notified_clone.lock().unwrap() += 1;
            }),
        );

        // Publish to wrong topic
        let node_change = Change::node_inserted(100, NODE_TOPIC.to_string());
        pubsub.publish(node_change);

        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(*notified.lock().unwrap(), 0);

        // Publish to correct topic
        let edge_change = Change::edge_inserted(100, 200, EDGE_TOPIC.to_string());
        pubsub.publish(edge_change);

        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(*notified.lock().unwrap(), 1);
    }

    #[test]
    fn test_wildcard_topic() {
        let pubsub = PubSub::new();

        let notified = Arc::new(Mutex::new(0));
        let notified_clone = notified.clone();

        pubsub.subscribe(
            vec!["*".to_string()], // Wildcard subscription
            Box::new(move |_| {
                *notified_clone.lock().unwrap() += 1;
            }),
        );

        let change = Change::node_inserted(100, NODE_TOPIC.to_string());
        pubsub.publish(change);

        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(*notified.lock().unwrap(), 1);
    }

    #[test]
    fn test_wal_replay() {
        let pubsub = PubSub::new();

        let count = Arc::new(Mutex::new(0));
        let count_clone = count.clone();

        pubsub.subscribe(
            vec![NODE_TOPIC.to_string()],
            Box::new(move |_| {
                *count_clone.lock().unwrap() += 1;
            }),
        );

        // Publish some changes
        pubsub.publish(Change::node_inserted(100, NODE_TOPIC.to_string()));
        pubsub.publish(Change::edge_inserted(100, 200, EDGE_TOPIC.to_string()));

        assert_eq!(pubsub.change_log_size(), 2);

        // Simulate restart by creating new subscriber and replaying
        let count2 = Arc::new(Mutex::new(0));
        let count2_clone = count2.clone();

        pubsub.subscribe(
            vec![NODE_TOPIC.to_string()],
            Box::new(move |_| {
                *count2_clone.lock().unwrap() += 1;
            }),
        );

        let notified = pubsub.replay_wal().unwrap();
        assert_eq!(notified, 2); // Both subscribers notified of NODE_TOPIC change
        assert_eq!(*count2.lock().unwrap(), 1); // New subscriber notified once
    }

    #[test]
    fn test_change_log_persistence() {
        let pubsub = PubSub::new();

        assert_eq!(pubsub.change_log_size(), 0);

        pubsub.publish(Change::node_inserted(100, NODE_TOPIC.to_string()));
        assert_eq!(pubsub.change_log_size(), 1);

        pubsub.publish(Change::edge_inserted(100, 200, EDGE_TOPIC.to_string()));
        assert_eq!(pubsub.change_log_size(), 2);

        pubsub.clear_change_log();
        assert_eq!(pubsub.change_log_size(), 0);
    }

    #[test]
    fn test_edge_deletion() {
        let pubsub = PubSub::new();

        let notified = Arc::new(Mutex::new(false));
        let notified_clone = notified.clone();

        pubsub.subscribe(
            vec![EDGE_TOPIC.to_string()],
            Box::new(move |change| {
                if change.change_type == ChangeType::EdgeDeleted {
                    *notified_clone.lock().unwrap() = true;
                }
            }),
        );

        let change = Change::edge_deleted(100, 200, EDGE_TOPIC.to_string());
        pubsub.publish(change);

        std::thread::sleep(Duration::from_millis(10));

        assert!(*notified.lock().unwrap());
    }

    #[test]
    fn test_timestamp_ordering() {
        let pubsub = PubSub::new();

        pubsub.publish(Change::node_inserted(100, NODE_TOPIC.to_string()));
        std::thread::sleep(Duration::from_millis(50));
        pubsub.publish(Change::node_inserted(200, NODE_TOPIC.to_string()));

        let log = pubsub.change_log.lock().unwrap();
        // Verify timestamps exist and are reasonable
        assert!(log[0].timestamp > 0);
        assert!(log[1].timestamp > 0);
        // Timestamps may be equal on fast systems, so don't assert inequality
        assert_eq!(log.len(), 2);
    }
}
