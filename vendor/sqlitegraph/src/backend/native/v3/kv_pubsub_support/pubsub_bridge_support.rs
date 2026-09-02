use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::PubSubEvent;
use crate::backend::SubscriptionFilter;
use crate::backend::native::v3::Publisher;
use crate::backend::native::v3::pubsub::types::{
    PubSubEvent as V3Event, SubscriberId, SubscriptionFilter as V3Filter,
};
use std::sync::mpsc::Receiver;

impl V3Backend {
    pub(crate) fn subscribe_pubsub(
        &self,
        filter: SubscriptionFilter,
    ) -> Result<(u64, Receiver<PubSubEvent>), SqliteGraphError> {
        let v3_filter = V3Filter {
            node_changes: filter.node_changes,
            edge_changes: filter.edge_changes,
            kv_changes: filter.kv_changes,
            snapshot_commits: filter.snapshot_commits,
        };

        let (sub_id, v3_rx) = {
            let mut pub_guard = self.publisher.write();
            pub_guard
                .get_or_insert_with(Publisher::new)
                .subscribe(v3_filter)
        };

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            while let Ok(v3_event) = v3_rx.recv() {
                let event = match v3_event {
                    V3Event::NodeChanged {
                        node_id,
                        snapshot_id,
                    } => PubSubEvent::NodeChanged {
                        node_id,
                        snapshot_id,
                    },
                    V3Event::EdgeChanged {
                        edge_id,
                        from_node,
                        to_node,
                        snapshot_id,
                    } => PubSubEvent::EdgeChanged {
                        edge_id,
                        from_node,
                        to_node,
                        snapshot_id,
                    },
                    V3Event::KvChanged {
                        key_hash,
                        snapshot_id,
                    } => PubSubEvent::KvChanged {
                        key_hash,
                        snapshot_id,
                    },
                    V3Event::SnapshotCommitted { snapshot_id } => {
                        PubSubEvent::SnapshotCommitted { snapshot_id }
                    }
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        });

        Ok((sub_id.as_u64(), rx))
    }

    pub(crate) fn unsubscribe_pubsub(&self, subscriber_id: u64) -> Result<bool, SqliteGraphError> {
        let pub_guard = self.publisher.read();
        let removed = pub_guard
            .as_ref()
            .map(|p| p.unsubscribe(SubscriberId::from_raw(subscriber_id)))
            .unwrap_or(false);
        Ok(removed)
    }
}
