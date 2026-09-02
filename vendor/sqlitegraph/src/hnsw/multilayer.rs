//! Multi-layer HNSW Implementation
//!
//! This module implements the multi-layer functionality for HNSW, providing
//! the dual-index mapping system that resolves node ID conflicts between
//! global vector storage (1-based) and layer-local node management (0-based).
//!
//! # Architecture
//!
//! The multi-layer system consists of:
//! - **LayerMappings**: Bidirectional mapping between global and local IDs
//! - **LevelDistributor**: Exponential distribution for level assignment
//! - **MultiLayerNodeManager**: Orchestration of multi-layer operations
//! - **Feature Flag**: Safe migration between single and multi-layer modes
//!
//! # Key Concepts
//!
//! ## Global vs Local IDs
//! - **Global IDs**: 1-based IDs from vector storage (1, 2, 3, ...)
//! - **Local IDs**: 0-based IDs within each layer (0, 1, 2, ...)
//! - **Bidirectional Mapping**: Efficient translation between the two systems
//!
//! ## Level Assignment
//! - **Exponential Distribution**: P(level = ℓ) = m^(-ℓ) where m is the base M parameter
//! - **Deterministic**: Uses seeded random number generator for reproducibility
//! - **Configurable**: Maximum layer limits and distribution parameters
//!
//! # Integration Strategy
//!
//! The multi-layer system integrates with existing HNSW components:
//! - Extends HnswConfig with multi-layer options
//! - Wraps existing HnswIndex with multi-layer orchestration
//! - Maintains full backward compatibility with single-layer mode
//! - Provides feature flags for safe migration

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::mem;

use crate::hnsw::{
    config::HnswConfig,
    errors::{HnswError, HnswMultiLayerError},
};

/// Bidirectional mapping system for multi-layer HNSW
///
/// Maintains translation between global vector IDs (1-based from storage)
/// and layer-local node IDs (0-based for each layer). This resolves the core
/// node ID conflict that was blocking multi-layer functionality.
///
/// # Data Structure
///
/// ```text
/// Global ID (1-based) → Local IDs per layer (0-based)
///     1 → [Some(0), None, Some(0)]  // Vector 1 in layers 0 and 2
///     2 → [Some(1), Some(0), None]  // Vector 2 in layers 0 and 1
///     3 → [None, Some(1), Some(1)]  // Vector 3 in layers 1 and 2
///
/// Local ID → Global ID per layer (0-based)
///     Layer 0: 0→1, 1→2
///     Layer 1: 0→2, 1→3
///     Layer 2: 0→1, 1→3
/// ```
#[derive(Debug, Clone)]
pub struct LayerMappings {
    /// Global ID → Vec<Option<u64>> mapping
    /// Index: VectorID (1-based), Value: Option<LocalID> per layer
    global_to_local: HashMap<u64, Vec<Option<u64>>>,

    /// Local ID → Global ID mapping per layer
    /// Index: Layer ID, Value: HashMap<LocalID, GlobalID>
    local_to_global: Vec<HashMap<u64, u64>>,

    /// Next available local ID for each layer
    next_local_id: Vec<usize>,
}

impl LayerMappings {
    /// Create new layer mappings with specified layer count
    ///
    /// # Arguments
    ///
    /// * `max_layers` - Maximum number of layers to support
    ///
    /// # Returns
    ///
    /// New LayerMappings instance ready for use
    pub fn new(max_layers: usize) -> Self {
        Self {
            global_to_local: HashMap::new(),
            local_to_global: (0..max_layers).map(|_| HashMap::new()).collect(),
            next_local_id: vec![0; max_layers],
        }
    }

    /// Add a mapping from global ID to local ID in specific layer
    ///
    /// # Arguments
    ///
    /// * `global_id` - Global vector ID (1-based)
    /// * `layer_id` - Layer ID (0-based)
    /// * `local_id` - Local node ID in the layer (0-based, None to auto-assign)
    ///
    /// # Returns
    ///
    /// Ok(()) if successful, Err if mapping conflicts
    pub fn add_mapping(
        &mut self,
        global_id: u64,
        layer_id: usize,
        local_id: Option<u64>,
    ) -> Result<(), HnswError> {
        // Ensure layer capacity
        if layer_id >= self.local_to_global.len() {
            self.extend_layers(layer_id + 1)?;
        }

        let local_id = local_id.unwrap_or_else(|| {
            let id = self.next_local_id[layer_id] as u64;
            self.next_local_id[layer_id] += 1;
            id
        });

        // Validate sequential local ID assignment
        if local_id != self.local_to_global[layer_id].len() as u64 {
            return Err(HnswError::MultiLayer(
                HnswMultiLayerError::LayerMappingConflict {
                    global_id,
                    layer_id,
                    local_id,
                    expected: self.local_to_global[layer_id].len() as u64,
                },
            ));
        }

        // Update global → local mapping
        let entry = self.global_to_local.entry(global_id).or_default();
        while entry.len() <= layer_id {
            entry.push(None);
        }
        entry[layer_id] = Some(local_id);

        // Update local → global mapping
        self.local_to_global[layer_id].insert(local_id, global_id);

        Ok(())
    }

    /// Get local ID for global ID in specific layer
    ///
    /// # Arguments
    ///
    /// * `global_id` - Global vector ID (1-based)
    /// * `layer_id` - Layer ID (0-based)
    ///
    /// # Returns
    ///
    /// Some(local_id) if mapping exists, None if not present
    pub fn get_local_id(&self, global_id: u64, layer_id: usize) -> Option<u64> {
        self.global_to_local
            .get(&global_id)
            .and_then(|mappings| mappings.get(layer_id).copied().flatten())
    }

    /// Get global ID for local ID in specific layer
    ///
    /// # Arguments
    ///
    /// * `layer_id` - Layer ID (0-based)
    /// * `local_id` - Local node ID (0-based)
    ///
    /// # Returns
    ///
    /// Some(global_id) if mapping exists, None if not present
    pub fn get_global_id(&self, layer_id: usize, local_id: u64) -> Option<u64> {
        self.local_to_global
            .get(layer_id)
            .and_then(|mapping| mapping.get(&local_id).copied())
    }

    /// Remove all mappings for a global ID
    ///
    /// # Arguments
    ///
    /// * `global_id` - Global vector ID to remove
    pub fn remove_global_id(&mut self, global_id: u64) -> Result<(), HnswError> {
        if let Some(mappings) = self.global_to_local.remove(&global_id) {
            // Remove reverse mappings
            for (layer_id, local_id) in mappings.iter().enumerate() {
                if let Some(id) = local_id {
                    self.local_to_global[layer_id].remove(id);
                }
            }
        }
        Ok(())
    }

    /// Get vector IDs that exist in a specific layer
    ///
    /// # Arguments
    ///
    /// * `layer_id` - Layer ID (0-based)
    ///
    /// # Returns
    ///
    /// Iterator over global IDs that have mappings in the layer
    pub fn get_layer_vectors(&self, layer_id: usize) -> Vec<u64> {
        if layer_id >= self.local_to_global.len() {
            return Vec::new();
        }

        let mut vectors: Vec<u64> = self.local_to_global[layer_id].values().copied().collect();
        vectors.sort(); // Sort for deterministic ordering
        vectors
    }

    /// Check if mappings are consistent
    ///
    /// # Returns
    ///
    /// Ok(()) if all mappings are consistent, Err with details if conflicts found
    pub fn validate_consistency(&self) -> Result<(), HnswError> {
        // Validate bidirectional consistency
        for (&global_id, mappings) in &self.global_to_local {
            for (layer_id, &local_id) in mappings.iter().enumerate() {
                if let Some(id) = local_id
                    && let Some(mapped_global) = self.get_global_id(layer_id, id)
                    && mapped_global != global_id
                {
                    return Err(HnswError::MultiLayer(
                        HnswMultiLayerError::InconsistentMapping {
                            global_id,
                            layer_id,
                            local_id: id,
                            mapped_global,
                        },
                    ));
                }
            }
        }

        // Validate reverse bidirectional consistency (local → global)
        for (layer_id, mapping) in self.local_to_global.iter().enumerate() {
            for (&local_id, &global_id) in mapping {
                if let Some(mapped_local) = self.get_local_id(global_id, layer_id) {
                    if mapped_local != local_id {
                        return Err(HnswError::MultiLayer(
                            HnswMultiLayerError::InconsistentMapping {
                                global_id,
                                layer_id,
                                local_id,
                                mapped_global: global_id, // We don't have a different global, so use the same
                            },
                        ));
                    }
                } else {
                    // Found a local→global mapping but no corresponding global→local mapping
                    return Err(HnswError::MultiLayer(
                        HnswMultiLayerError::InconsistentMapping {
                            global_id,
                            layer_id,
                            local_id,
                            mapped_global: u64::MAX, // Use sentinel value to indicate missing reverse mapping
                        },
                    ));
                }
            }
        }

        // Validate sequential local IDs per layer
        for (layer_id, mapping) in self.local_to_global.iter().enumerate() {
            let expected_count = mapping.len();
            if expected_count != self.next_local_id[layer_id] {
                return Err(HnswError::MultiLayer(
                    HnswMultiLayerError::InconsistentLayerState {
                        layer_id,
                        expected_nodes: expected_count,
                        actual_nodes: mapping.len(),
                    },
                ));
            }
        }

        Ok(())
    }

    /// Get memory usage estimate in bytes
    ///
    /// # Returns
    ///
    /// Estimated memory usage for the mapping data structures
    pub fn memory_usage(&self) -> usize {
        let base_overhead = mem::size_of::<Self>();

        let global_to_local_size = self.global_to_local.len()
            * (mem::size_of::<u64>() + // key
            mem::size_of::<Vec<Option<u64>>>() + // value type overhead
            self.global_to_local.values()
                .map(|v| v.len() * mem::size_of::<Option<u64>>())
                .sum::<usize>());

        let local_to_global_size = self
            .local_to_global
            .iter()
            .map(|m| m.len() * (mem::size_of::<u64>() + mem::size_of::<u64>()))
            .sum::<usize>();

        let next_local_id_size = self.next_local_id.len() * mem::size_of::<usize>();

        base_overhead + global_to_local_size + local_to_global_size + next_local_id_size
    }

    /// Clear all mappings
    pub fn clear(&mut self) {
        self.global_to_local.clear();
        for mapping in &mut self.local_to_global {
            mapping.clear();
        }
        for next_id in &mut self.next_local_id {
            *next_id = 0;
        }
    }

    /// Extend the number of layers supported
    fn extend_layers(&mut self, required_layers: usize) -> Result<(), HnswError> {
        let current_layers = self.local_to_global.len();
        if required_layers <= current_layers {
            return Ok(());
        }

        // Add new layers
        for _ in current_layers..required_layers {
            self.local_to_global.push(HashMap::new());
            self.next_local_id.push(0);
        }

        // Extend existing mappings to include new layers
        for mappings in self.global_to_local.values_mut() {
            while mappings.len() < required_layers {
                mappings.push(None);
            }
        }

        Ok(())
    }
}

/// Exponential level distributor for multi-layer HNSW
///
/// Implements the probabilistic level assignment algorithm from the original
/// HNSW paper. Elements are inserted into layer ℓ with probability m^(-ℓ),
/// creating a natural hierarchy where higher layers contain fewer elements.
///
/// # Mathematical Properties
///
/// For M connections per node:
/// - P(level = 0) = 1.0 (always insert into base layer)
/// - P(level = 1) = 1/M (insert into level 1 with 1/M probability)
/// - P(level = 2) = 1/M² (insert into level 2 with 1/M² probability)
/// - ...and so on
///
/// # Determinism
///
/// Uses a seeded random number generator to ensure reproducible level assignment
/// across different runs and environments.
#[derive(Debug, Clone)]
pub struct LevelDistributor {
    /// Base M parameter for exponential distribution
    base_m: f64,

    /// Maximum number of levels
    max_layers: usize,

    /// Seeded random number generator
    rng: StdRng,
}

impl LevelDistributor {
    /// Create new level distributor
    ///
    /// # Arguments
    ///
    /// * `base_m` - Base M parameter (typically 16)
    /// * `max_layers` - Maximum level count (typically 16)
    /// * `seed` - Random seed for deterministic behavior
    ///
    /// # Returns
    ///
    /// New LevelDistributor instance
    pub fn new(base_m: f64, max_layers: usize) -> Self {
        Self {
            base_m,
            max_layers,
            rng: StdRng::from_entropy(), // Will be seeded by config
        }
    }

    /// Set random seed for deterministic behavior
    ///
    /// # Arguments
    ///
    /// * `seed` - 64-bit seed value
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.rng = StdRng::seed_from_u64(seed);
        self
    }

    /// Sample a level using exponential distribution
    ///
    /// # Arguments
    ///
    /// * `rng` - Random number generator (or use internal RNG if None)
    ///
    /// # Returns
    ///
    /// Level assignment (0..max_layers-1)
    pub fn sample_level(&mut self, rng: Option<&mut impl Rng>) -> usize {
        let mut level = 0;

        if let Some(rng) = rng {
            // Exponential distribution: P(level = ℓ) = m^(-ℓ)
            while rng.r#gen::<f64>() < 1.0 / self.base_m && level < self.max_layers - 1 {
                level += 1;
            }
        } else {
            // Use internal RNG
            while self.rng.r#gen::<f64>() < 1.0 / self.base_m && level < self.max_layers - 1 {
                level += 1;
            }
        }

        level
    }

    /// Get probability of inserting into specific level
    ///
    /// # Arguments
    ///
    /// * `level` - Level to calculate probability for
    ///
    /// # Returns
    ///
    /// Probability value (0.0..1.0)
    pub fn level_probability(&self, level: usize) -> f64 {
        if level >= self.max_layers {
            return 0.0;
        }
        self.base_m.powf(-(level as f64))
    }

    /// Expected number of vectors per level for given dataset size
    ///
    /// # Arguments
    ///
    /// * `total_vectors` - Total number of vectors
    /// * `level` - Level to calculate expectation for
    ///
    /// # Returns
    ///
    /// Expected vector count for the level
    pub fn expected_vectors_at_level(&self, total_vectors: usize, level: usize) -> f64 {
        if level >= self.max_layers {
            return 0.0;
        }

        let prob = self.level_probability(level);
        total_vectors as f64 * prob
    }

    /// Get all level probabilities for current configuration
    ///
    /// # Returns
    ///
    /// Vector of probabilities for levels 0..max_layers-1
    pub fn all_level_probabilities(&self) -> Vec<f64> {
        (0..self.max_layers)
            .map(|level| self.level_probability(level))
            .collect()
    }

    /// Level assignment using internal RNG only (convenience method)
    ///
    /// # Returns
    ///
    /// Level assignment (0..max_layers-1)
    pub fn sample_level_internal(&mut self) -> usize {
        let mut level = 0;
        while self.rng.r#gen::<f64>() < 1.0 / self.base_m && level < self.max_layers - 1 {
            level += 1;
        }
        level
    }
}

/// Multi-layer node manager that orchestrates layer operations
///
/// Provides high-level interface for multi-layer HNSW functionality, coordinating
/// between vector storage, layer management, and search operations.
/// Wraps existing single-layer components to provide multi-layer capabilities.
///
/// # Key Responsibilities
///
/// - Coordinate insertions across multiple layers
/// - Manage bidirectional ID mappings
/// - Orchestrate multi-layer search operations
/// - Provide feature flag for safe migration
#[derive(Debug)]
pub struct MultiLayerNodeManager {
    /// Layer mappings for ID translation
    mappings: LayerMappings,

    /// Layer distributor for level assignment
    distributor: LevelDistributor,

    /// Multi-layer configuration
    config: HnswConfig,

    /// Layer assignments per vector (for debugging and statistics)
    vector_levels: HashMap<u64, usize>,
}

impl MultiLayerNodeManager {
    /// Create new multi-layer node manager
    ///
    /// # Arguments
    ///
    /// * `config` - HNSW configuration
    ///
    /// # Returns
    ///
    /// New MultiLayerNodeManager instance
    pub fn new(config: HnswConfig) -> Result<Self, HnswError> {
        let max_layers = config.ml as usize;
        let mappings = LayerMappings::new(max_layers);
        // Apply the config seed + distribution base so multilayer level
        // assignment is deterministic. Without this the distributor defaults to
        // StdRng::from_entropy() (see LevelDistributor::new), making layer
        // counts vary across process invocations even when
        // `multilayer_deterministic_seed` is set — the cause of the
        // `test_multilayer_insert_layers_correct` flakiness. This mirrors the
        // (previously dead-code) seeding in HnswIndex::with_storage.
        let base_m = config
            .multilayer_level_distribution_base
            .unwrap_or(config.m) as f64;
        let seed = config.multilayer_deterministic_seed.unwrap_or(42);
        let distributor = LevelDistributor::new(base_m, max_layers).with_seed(seed);
        let vector_levels = HashMap::new();

        Ok(Self {
            mappings,
            distributor,
            config,
            vector_levels,
        })
    }

    /// Insert vector into appropriate layers
    ///
    /// # Arguments
    ///
    /// * `vector_id` - Global vector ID from storage
    ///
    /// # Returns
    ///
    /// Tuple of (highest_level, layer_assignments) where layer_assignments
    /// is a vector of (layer_id, local_id) pairs
    pub fn insert_vector(
        &mut self,
        vector_id: u64,
    ) -> Result<(usize, Vec<(usize, u64)>), HnswError> {
        // Determine insertion level using exponential distribution
        let highest_level = self.distributor.sample_level_internal();

        let mut layer_assignments = Vec::new();

        // Insert into all layers from highest_level down to base layer
        for level in (0..=highest_level).rev() {
            let local_id = self.mappings.next_local_id[level]; // Get current count before assignment
            self.mappings.add_mapping(vector_id, level, None)?; // Auto-assign sequential ID (increments counter)
            layer_assignments.push((level, local_id as u64));
        }

        // Track vector level assignment
        self.vector_levels.insert(vector_id, highest_level);

        Ok((highest_level, layer_assignments))
    }

    /// Remove vector from all layers
    ///
    /// # Arguments
    ///
    /// * `vector_id` - Global vector ID to remove
    pub fn remove_vector(&mut self, vector_id: u64) -> Result<(), HnswError> {
        // Remove from mapping system
        self.mappings.remove_global_id(vector_id)?;
        self.vector_levels.remove(&vector_id);

        Ok(())
    }

    /// Get local ID for global ID in specific layer
    ///
    /// # Arguments
    ///
    /// * `vector_id` - Global vector ID
    /// * `layer_id` - Layer ID
    ///
    /// # Returns
    ///
    /// Local node ID if mapping exists
    pub fn get_local_id(&self, vector_id: u64, layer_id: usize) -> Option<u64> {
        self.mappings.get_local_id(vector_id, layer_id)
    }

    /// Get global ID for local ID in specific layer
    ///
    /// # Arguments
    ///
    /// * `layer_id` - Layer ID
    /// * `local_id` - Local node ID
    ///
    /// # Returns
    ///
    /// Global vector ID if mapping exists
    pub fn get_global_id(&self, layer_id: usize, local_id: u64) -> Option<u64> {
        self.mappings.get_global_id(layer_id, local_id)
    }

    /// Get vectors assigned to specific layer
    ///
    /// # Arguments
    ///
    /// * `layer_id` - Layer ID
    ///
    /// # Returns
    ///
    /// Iterator over global IDs in the layer
    pub fn get_layer_vectors(&self, layer_id: usize) -> Vec<u64> {
        self.mappings.get_layer_vectors(layer_id)
    }

    /// Get highest level assignment for a vector
    ///
    /// # Arguments
    ///
    /// * `vector_id` - Global vector ID
    ///
    /// # Returns
    ///
    /// Highest layer where vector exists, or None if not inserted
    pub fn get_vector_level(&self, vector_id: u64) -> Option<usize> {
        self.vector_levels.get(&vector_id).copied()
    }

    /// Get statistics about layer distribution
    ///
    /// # Returns
    ///
    /// Tuple of (total_vectors, layer_counts, memory_usage)
    pub fn get_statistics(&self) -> (usize, Vec<usize>, usize) {
        let total_vectors = self.vector_levels.len();
        let max_layers = self.config.ml as usize;
        let mut layer_counts = vec![0; max_layers];

        // Count vectors in each layer
        for (level, count) in layer_counts.iter_mut().enumerate().take(max_layers) {
            *count = self.mappings.get_layer_vectors(level).len();
        }

        let memory_usage = self.mappings.memory_usage();

        (total_vectors, layer_counts, memory_usage)
    }

    /// Validate all mappings and consistency
    ///
    /// # Returns
    ///
    /// Ok(()) if all mappings are consistent
    pub fn validate_consistency(&self) -> Result<(), HnswError> {
        self.mappings.validate_consistency()
    }

    /// Clear all mappings and start fresh
    pub fn clear(&mut self) -> Result<(), HnswError> {
        self.mappings.clear();
        self.vector_levels.clear();
        Ok(())
    }
}

/// Multi-layer error types for enhanced error reporting
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hnsw::hnsw_config;
    use rand::rngs::StdRng;

    #[test]
    fn test_layer_mappings_basic_operations() {
        let mut mappings = LayerMappings::new(3);

        // Test basic mapping addition
        mappings.add_mapping(1, 0, Some(0)).unwrap();
        mappings.add_mapping(1, 1, Some(0)).unwrap();
        mappings.add_mapping(2, 0, Some(1)).unwrap();

        // Test retrieval
        assert_eq!(mappings.get_local_id(1, 0), Some(0));
        assert_eq!(mappings.get_local_id(1, 1), Some(0));
        assert_eq!(mappings.get_local_id(2, 0), Some(1));
        assert_eq!(mappings.get_local_id(3, 0), None);

        // Test reverse mapping
        assert_eq!(mappings.get_global_id(0, 0), Some(1));
        assert_eq!(mappings.get_global_id(0, 1), Some(2));
        assert_eq!(mappings.get_global_id(1, 0), Some(1));
    }

    #[test]
    fn test_layer_mappings_sequential_assignment() {
        let mut mappings = LayerMappings::new(2);

        // Test automatic local ID assignment
        mappings.add_mapping(1, 0, None).unwrap(); // Should assign 0
        mappings.add_mapping(2, 0, None).unwrap(); // Should assign 1
        mappings.add_mapping(3, 0, None).unwrap(); // Should assign 2

        assert_eq!(mappings.get_local_id(1, 0), Some(0));
        assert_eq!(mappings.get_local_id(2, 0), Some(1));
        assert_eq!(mappings.get_layer_vectors(0), vec![1, 2, 3]);
    }

    #[test]
    fn test_layer_mappings_sequential_violation() {
        let mut mappings = LayerMappings::new(2);

        mappings.add_mapping(1, 0, Some(0)).unwrap();

        // This should fail - trying to assign local ID 1 instead of sequential 1
        let result = mappings.add_mapping(2, 0, Some(2));
        assert!(result.is_err());

        // Verify no mapping was added
        assert_eq!(mappings.get_local_id(2, 0), None);
    }

    #[test]
    fn test_level_distributor_deterministic() {
        let seed = 42u64;
        let mut distributor1 = LevelDistributor::new(16.0, 5).with_seed(seed);
        let mut distributor2 = LevelDistributor::new(16.0, 5).with_seed(seed);

        let mut counts1 = vec![0; 5];
        let mut counts2 = vec![0; 5];

        for _ in 0..1000 {
            counts1[distributor1.sample_level(None::<&mut StdRng>)] += 1;
            counts2[distributor2.sample_level(None::<&mut StdRng>)] += 1;
        }

        // Results should be identical with same seed
        assert_eq!(counts1, counts2);
    }

    #[test]
    fn test_level_distributor_mathematical_properties() {
        let distributor = LevelDistributor::new(16.0, 4);

        // Test probability calculations
        assert_eq!(distributor.level_probability(0), 1.0);
        assert_eq!(distributor.level_probability(1), 1.0 / 16.0);
        assert_eq!(distributor.level_probability(2), 1.0 / 256.0);
        assert_eq!(distributor.level_probability(3), 1.0 / 4096.0);

        // Test expected distribution
        let total_vectors = 10000;
        let expected_l0 = distributor.expected_vectors_at_level(total_vectors, 0);
        let expected_l1 = distributor.expected_vectors_at_level(total_vectors, 1);
        let expected_l2 = distributor.expected_vectors_at_level(total_vectors, 2);

        assert!(expected_l0 > expected_l1);
        assert!(expected_l1 > expected_l2);

        // Note: In HNSW, each vector appears in ALL levels from 0 up to its assigned level.
        // So the total number of "slots" needed > total vectors.
        // For M=16, expected total slots = total * (1 + 1/M + 1/M^2 + 1/M^3) ≈ total * (M/(M-1))
        let expected_total_slots = total_vectors as f64 * (16.0 / 15.0); // ≈ 10666.7 for 10000 vectors
        let sum = expected_l0 + expected_l1 + expected_l2;
        assert!((sum - expected_total_slots).abs() < 200.0); // Allow reasonable tolerance
    }

    #[test]
    fn test_multilayer_node_manager_basic_operations() {
        let config = hnsw_config()
            .m_connections(16)
            .max_layers(8)
            .build()
            .unwrap();

        let mut manager = MultiLayerNodeManager::new(config).unwrap();

        // Test vector insertion
        let (level1, assignments1) = manager.insert_vector(1).unwrap();
        let (level2, assignments2) = manager.insert_vector(2).unwrap();
        let (level3, assignments3) = manager.insert_vector(3).unwrap();

        assert!(level1 <= 7); // max_layers-1

        // Verify each vector is in all required layers
        assert_eq!(assignments1.len(), level1 + 1); // levels 0..level1
        assert_eq!(assignments2.len(), level2 + 1); // layers 0..level2
        assert_eq!(assignments3.len(), level3 + 1); // layers 0..level3

        // Verify sequential local IDs within each layer
        for (_level, local_id) in assignments1.iter() {
            let expected_local_id = 0; // First vector in each layer gets ID = 0
            assert_eq!(*local_id, expected_local_id as u64);
        }
    }

    #[test]
    fn test_multilayer_node_manager_statistics() {
        let config = hnsw_config().max_layers(4).build().unwrap();

        let mut manager = MultiLayerNodeManager::new(config).unwrap();

        // Insert vectors
        for i in 1..=20 {
            manager.insert_vector(i).unwrap();
        }

        let (total, layer_counts, memory) = manager.get_statistics();

        assert_eq!(total, 20);
        assert_eq!(layer_counts.len(), 4);
        assert!(layer_counts[0] >= layer_counts[1]);
        assert!(layer_counts[1] >= layer_counts[2]);
        assert!(layer_counts[2] >= layer_counts[3]);

        // All vectors should be in base layer
        assert_eq!(layer_counts[0], 20);

        // Memory usage should be reasonable
        assert!(memory > 0);
    }

    #[test]
    fn test_multilayer_node_manager_consistency() {
        let config = hnsw_config().build().unwrap();

        let mut manager = MultiLayerNodeManager::new(config).unwrap();

        // Add some mappings
        manager.insert_vector(1).unwrap();
        manager.insert_vector(2).unwrap();
        manager.insert_vector(3).unwrap();

        // Consistency should pass
        assert!(manager.validate_consistency().is_ok());

        // Break consistency manually by removing a mapping from layer 0
        // (Most vectors will be in layer 0 with exponential distribution)
        manager.mappings.local_to_global[0].remove(&1);

        // Consistency should now fail
        assert!(manager.validate_consistency().is_err());
    }

    #[test]
    fn test_multilayer_node_manager_removal() {
        let config = hnsw_config().build().unwrap();

        let mut manager = MultiLayerNodeManager::new(config).unwrap();

        // Insert vectors
        let _id1 = manager.insert_vector(1).unwrap();
        let _id2 = manager.insert_vector(2).unwrap();
        let _id3 = manager.insert_vector(3).unwrap();

        // Remove one vector
        manager.remove_vector(2).unwrap();

        // Verify mapping removal
        assert_eq!(manager.get_local_id(2, 0), None);
        assert_eq!(manager.get_local_id(1, 0), Some(0));
        assert_eq!(manager.get_local_id(3, 0), Some(2));

        // Verify statistics
        let (total, _, _) = manager.get_statistics();
        assert_eq!(total, 2); // Should be 2 remaining vectors
    }
}
