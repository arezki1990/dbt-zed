//! V3 Native Pub/Sub - Core Types
//!
//! This module defines the types for V3's native pub/sub system,
//! designed for in-process event notification.

use serde::{Deserialize, Serialize};

/// Unique identifier for a subscriber
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubscriberId(u64);

impl SubscriberId {
    /// Create a new subscriber ID
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the raw ID value
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    /// Create from raw u64
    pub fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

impl Default for SubscriberId {
    fn default() -> Self {
        Self(1)
    }
}

/// Types of pub/sub events
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PubSubEventType {
    /// Node created or modified
    NodeChanged = 1,
    /// Edge created or modified
    EdgeChanged = 2,
    /// KV entry created, modified, or deleted
    KvChanged = 3,
    /// Transaction committed
    SnapshotCommitted = 4,
}

/// Event delivered to subscribers
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PubSubEvent {
    /// Node created or modified
    NodeChanged {
        /// Node ID that changed
        node_id: i64,
        /// Snapshot ID when change was committed
        snapshot_id: u64,
    },
    /// Edge created or modified
    EdgeChanged {
        /// Edge ID that changed
        edge_id: i64,
        /// Source node ID
        from_node: i64,
        /// Target node ID
        to_node: i64,
        /// Snapshot ID when change was committed
        snapshot_id: u64,
    },
    /// KV entry changed
    KvChanged {
        /// Key hash for the KV entry
        key_hash: u64,
        /// Snapshot ID when change was committed
        snapshot_id: u64,
    },
    /// Transaction committed
    SnapshotCommitted {
        /// Snapshot ID that was committed
        snapshot_id: u64,
    },
}

impl PubSubEvent {
    /// Get the event type
    pub fn event_type(&self) -> PubSubEventType {
        match self {
            Self::NodeChanged { .. } => PubSubEventType::NodeChanged,
            Self::EdgeChanged { .. } => PubSubEventType::EdgeChanged,
            Self::KvChanged { .. } => PubSubEventType::KvChanged,
            Self::SnapshotCommitted { .. } => PubSubEventType::SnapshotCommitted,
        }
    }

    /// Get the snapshot ID for this event
    pub fn snapshot_id(&self) -> u64 {
        match self {
            Self::NodeChanged { snapshot_id, .. } => *snapshot_id,
            Self::EdgeChanged { snapshot_id, .. } => *snapshot_id,
            Self::KvChanged { snapshot_id, .. } => *snapshot_id,
            Self::SnapshotCommitted { snapshot_id } => *snapshot_id,
        }
    }
}

/// Filter for subscriptions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SubscriptionFilter {
    /// Subscribe to NodeChanged events
    pub node_changes: bool,
    /// Subscribe to EdgeChanged events
    pub edge_changes: bool,
    /// Subscribe to KvChanged events
    pub kv_changes: bool,
    /// Subscribe to SnapshotCommitted events
    pub snapshot_commits: bool,
}

impl SubscriptionFilter {
    /// Create a filter that receives all event types
    pub fn all() -> Self {
        Self {
            node_changes: true,
            edge_changes: true,
            kv_changes: true,
            snapshot_commits: true,
        }
    }

    /// Create a filter for node changes only
    pub fn nodes_only() -> Self {
        Self {
            node_changes: true,
            ..Default::default()
        }
    }

    /// Create a filter for edge changes only
    pub fn edges_only() -> Self {
        Self {
            edge_changes: true,
            ..Default::default()
        }
    }

    /// Create a filter for KV changes only
    pub fn kv_only() -> Self {
        Self {
            kv_changes: true,
            ..Default::default()
        }
    }

    /// Check if an event matches this filter
    pub fn matches(&self, event: &PubSubEvent) -> bool {
        match event {
            PubSubEvent::NodeChanged { .. } => self.node_changes,
            PubSubEvent::EdgeChanged { .. } => self.edge_changes,
            PubSubEvent::KvChanged { .. } => self.kv_changes,
            PubSubEvent::SnapshotCommitted { .. } => self.snapshot_commits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscriber_id() {
        let id = SubscriberId::new(42);
        assert_eq!(id.as_u64(), 42);
        assert_eq!(SubscriberId::from_raw(42).as_u64(), 42);
    }

    #[test]
    fn test_subscription_filter_all() {
        let filter = SubscriptionFilter::all();
        assert!(filter.node_changes);
        assert!(filter.edge_changes);
        assert!(filter.kv_changes);
        assert!(filter.snapshot_commits);
    }

    #[test]
    fn test_subscription_filter_matches() {
        let filter = SubscriptionFilter::nodes_only();
        assert!(filter.matches(&PubSubEvent::NodeChanged {
            node_id: 1,
            snapshot_id: 1
        }));
        assert!(!filter.matches(&PubSubEvent::EdgeChanged {
            edge_id: 1,
            from_node: 1,
            to_node: 2,
            snapshot_id: 1
        }));
    }

    #[test]
    fn test_pubsub_event_types() {
        let node_event = PubSubEvent::NodeChanged {
            node_id: 1,
            snapshot_id: 1,
        };
        assert_eq!(node_event.event_type(), PubSubEventType::NodeChanged);
        assert_eq!(node_event.snapshot_id(), 1);

        let kv_event = PubSubEvent::KvChanged {
            key_hash: 123,
            snapshot_id: 5,
        };
        assert_eq!(kv_event.event_type(), PubSubEventType::KvChanged);
        assert_eq!(kv_event.snapshot_id(), 5);
    }
}
