//! V3 Native Pub/Sub System
//!
//! This module provides an in-process pub/sub system for V3,
//! designed for event notification on transaction commits.
//!
//! ## Architecture
//!
//! - **Channel-based delivery**: Uses `std::sync::mpsc` for in-process delivery
//! - **Best-effort**: Events are dropped if channel is full or receiver is gone
//! - **Sync emit**: Called on commit path, no background threads
//! - **Filter-based**: Subscribers specify which event types they want
//!
//! ## Event Types
//!
//! - `NodeChanged` - Node created or modified
//! - `EdgeChanged` - Edge created or modified  
//! - `KvChanged` - KV entry changed
//! - `SnapshotCommitted` - Transaction committed

pub mod publisher;
pub mod types;

// Re-export public API
pub use publisher::Publisher;
pub use types::{PubSubEvent, PubSubEventType, SubscriberId, SubscriptionFilter};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        let _publisher = Publisher::new();
        let _filter = SubscriptionFilter::all();
        let _event = PubSubEvent::SnapshotCommitted { snapshot_id: 1 };
    }
}
