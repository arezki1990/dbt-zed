//! CSR sharding for prompt-local subgraph traversal.
//!
//! Sharded graph traversal loads only the shards needed for a given prompt,
//! enabling subgraph construction without full-graph traversal overhead.

mod bidirectional;
mod filters_spec;
mod fts5_spec;
mod manifest;
mod property;
mod pubsub;
mod reader;
mod semantic;
mod shard;
mod streams_spec;
mod subgraph;

#[cfg(test)]
mod data_tests;
#[cfg(test)]
mod integration_tests;

pub use bidirectional::BidirectionalIndex;
pub use filters_spec::{AttrFilter, Attribute};
pub use fts5_spec::ContentMatch;
pub use manifest::{Manifest, ShardMetadata};
pub use property::PropertyStore;
pub use pubsub::{Change, ChangeType, DEFAULT_TOPIC, EDGE_TOPIC, NODE_TOPIC, PubSub, Subscriber};
pub use reader::ShardReader;
pub use semantic::{KnnResult, SemanticLayer};
pub use shard::{CsrEdge, CsrShard};
pub use streams_spec::ConsumerGroup;
pub use subgraph::{Subgraph, SubgraphBuilder};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify public API exports
        let _manifest = Manifest::new(vec![], "1.0".to_string());
        let _shard = CsrShard::new(0, 1000, 2000);
    }
}
