# HNSW Development Rules & Process

**Date**: 2025-12-19
**Scope**: HNSW Vector Integration Development
**Status**: ACTIVE - Must be followed strictly

---

## 🚫 **ABSOLUTE PROHIBITIONS**

1. **NO MOCKS** - Ever. No exceptions.
2. **NO STUBS** - Ever. No exceptions.
3. **NO PLACEHOLDERS** - Use real implementations only
4. **NO SIMULATION** - Real data and real behavior only
5. **NO PASS/FALLBACK** - Implement actual functionality
6. **NO ASSUMPTIONS** - Research, verify, then implement
7. **NO GUESSING** - Use proven algorithms and tested approaches

---

## ✅ **MANDATORY REQUIREMENTS**

### 1. **File Organization Rules**

```rust
// Every source file must be ≤ 1,000 LOC
// Every file must have comprehensive documentation
// Every public method must have doc comments
// Every function call must be traceable
// Every variable name must be meaningful
// Every error condition must be handled
```

### 2. **TDD Workflow Rules**

```
1. Write FAILING test first
2. Write MINIMAL code to make test pass
3. Refactor ONLY after test passes
4. NO code without corresponding test
5. Integration tests before implementation
6. Regression tests for every feature
```

### 3. **Documentation Requirements**

```rust
/// HNSW Vector Search Module
///
/// This module provides vector search capabilities
/// using Hierarchical Navigable Small World (HNSW) algorithm.
///
/// ## Architecture
/// - Separates index construction from search operations
/// - Uses SIMD-optimized distance calculations
/// - Supports both SQLite and Native V2 backends
///
/// ## Performance Characteristics
/// - Search: O(log N) average case
/// - Insert: O(log N) with dynamic updates
/// - Memory: 2-3x vector size overhead
///
/// ## Usage Examples
/// ```rust
/// let hnsw = HnswIndex::new(config)?;
/// hnsw.insert(vector, id)?;
/// let results = hnsw.search(query, k)?;
/// ```
pub struct HnswIndex {
    /// Core HNSW data structure
    /// Stores layered graph with connections
    layers: Vec<HnswLayer>,

    /// Entry point for searches
    /// ID of the node at top layer
    entry_point: u64,

    /// HNSW algorithm parameters
    /// Number of connections per layer
    config: HnswConfig,
}
```

### 4. **TodoWrite Tool Usage Rules**

```
For EVERY source file created:
1. Create specific TodoWrite for that file
2. Track all tasks for that file in its TodoWrite
3. Mark tasks as in_progress before starting
4. Mark tasks as completed only when DONE
5. No generic todos - file-specific only
```

### 5. **Integration Rules**

```
1. Must use existing SQLiteGraph patterns
2. Must follow SQLiteGraph error handling
3. Must use SQLiteGraph trait abstractions
4. Must maintain API consistency
5. No external dependencies without review
```

---

## 🏗️ **Module Structure Rules**

### 1. **Folder Organization**
```
sqlitegraph/src/hnsw/
├── mod.rs                 # Public API, ≤ 1,000 LOC
├── index.rs              # HNSW index management, ≤ 1,000 LOC
├── search.rs             # Search algorithms, ≤ 1,000 LOC
├── construction.rs       # Index construction, ≤ 1,000 LOC
├── distance.rs           # SIMD distance calculations, ≤ 1,000 LOC
├── storage.rs            # Vector storage abstraction, ≤ 1,000 LOC
├── config.rs             # Configuration types, ≤ 1,000 LOC
├── errors.rs             # HNSW-specific errors, ≤ 1,000 LOC
└── tests/
    ├── integration.rs    # Integration tests
    ├── regression.rs     # Regression tests
    └── performance.rs    # Performance benchmarks
```

### 2. **File Size Enforcement**
```
- Every .rs file ≤ 1,000 LOC (including comments/tests)
- No exceptions to 1,000 LOC limit
- Split files if exceeding limit
- Each file must have single responsibility
```

### 3. **Separation of Concerns**
```
index.rs        - Index lifecycle management
search.rs       - Search algorithm implementation
construction.rs - Index building algorithms
distance.rs     - Distance metric calculations only
storage.rs      - Abstract storage interface only
config.rs       - Configuration and parameters only
errors.rs       - Error types and handling only
```

---

## 🧪 **Testing Requirements**

### 1. **Test-Driven Development Sequence**
```rust
// STEP 1: Write failing test
#[test]
fn test_hnsw_basic_search() {
    let hnsw = HnswIndex::new(test_config());
    // Test will fail - no implementation yet
    assert!(hnsw.search(&test_vector(), 10).is_ok());
}

// STEP 2: Implement minimal code
pub struct HnswIndex {
    // Minimal implementation to pass test
}

// STEP 3: Run test - it should pass
// STEP 4: Refactor and improve
```

### 2. **Integration Test Requirements**
```rust
// Before ANY implementation, write integration tests
#[test]
fn test_sqlitegraph_hnsw_integration() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    // Test the complete integration workflow
    let result = graph.add_entity_embedding(1, test_vector()).unwrap();
    assert!(result.is_ok());
}
```

### 3. **Regression Test Requirements**
```rust
// Every feature must have regression tests
#[test]
fn test_hnsw_search_regression_2024_12_19() {
    // Specific test that catches known regressions
    // Must be updated if behavior intentionally changes
    let hnsw = setup_regression_test();
    let results = hnsw.search(&REGRESSION_QUERY, 10).unwrap();
    assert_eq!(results.len(), REGRESSION_EXPECTED_RESULTS);
}
```

---

## 🔧 **Implementation Rules**

### 1. **No Assumptions Rule**
```rust
// ❌ WRONG - Assuming hnsw-rs API exists
extern crate hnsw_rs;

// ✅ RIGHT - Research and verify first
use hnsw_rs::HnswIndex; // Only if crate actually exists and is verified

// ❌ WRONG - Assuming algorithm works
fn search_hnsw(query: &[f32]) -> Vec<u64> {
    // Magic implementation without understanding
}

// ✅ RIGHT - Understand algorithm first
fn search_hnsw(query: &[f32]) -> Result<Vec<u64>, HnswError> {
    // Implementation based on HNSW research papers
    // With proper error handling and edge cases
}
```

### 2. **No Mocks Rule**
```rust
// ❌ WRONG - Using mocks
#[cfg(test)]
mod mock_hnsw {
    pub struct MockHnswIndex;
    impl MockHnswIndex {
        pub fn search(&self, _query: &[f32]) -> Vec<u64> {
            vec![1, 2, 3] // Fake return
        }
    }
}

// ✅ RIGHT - Use real implementations
#[test]
fn test_hnsw_search() {
    let hnsw = HnswIndex::new(test_config()).unwrap();
    hnsw.insert(test_vector(), 1).unwrap();
    let results = hnsw.search(test_query(), 10).unwrap();
    assert!(!results.is_empty());
}
```

### 3. **Real Data Only Rule**
```rust
// ❌ WRONG - Fake/test data
let fake_embedding = vec![0.1, 0.2, 0.3];

// ✅ RIGHT - Real embeddings or realistic test data
let real_embedding = generate_test_embedding(768); // Actually generates
```

---

## 📝 **Documentation Requirements**

### 1. **Module Documentation**
```rust
//! Hierarchical Navigable Small World (HNSW) Vector Search
//!
//! This module provides an HNSW implementation with:
//! - SIMD-optimized distance calculations
//! - Multi-threaded index construction
//! - Dynamic updates without full rebuilds
//! - Memory-mapped persistence
//!
//! # Quick Start
//!
//! ```rust
//! use sqlitegraph::hnsw::{HnswIndex, HnswConfig};
//!
//! let config = HnswConfig::builder()
//!     .dimension(768)
//!     .m_connections(16)
//!     .ef_construction(200)
//!     .build()?;
//!
//! let mut hnsw = HnswIndex::new(config)?;
//! hnsw.insert(embedding, id)?;
//! let results = hnsw.search(query, 10)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
```

### 2. **Function Documentation**
```rust
/// Insert a vector into the HNSW index
///
/// This method inserts a new vector with the specified identifier into the HNSW
/// structure. The insertion process follows the HNSW algorithm:
///
/// 1. Determine appropriate layers for the new node
/// 2. Search for neighbors in each layer
/// 3. Establish connections following M parameter constraints
/// 4. Update entry point if necessary
///
/// # Arguments
///
/// * `vector` - The vector to insert, must match configured dimension
/// * `id` - Unique identifier for this vector
///
/// # Returns
///
/// Returns `Ok(())` on successful insertion or `HnswError` if:
/// - Vector dimension doesn't match configuration
/// - ID already exists in index
/// - Index construction failed
///
/// # Examples
///
/// ```rust
/// let embedding = vec![0.1; 768];
/// hnsw.insert(embedding, 42)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Performance
///
/// - Time Complexity: O(log N) average case
/// - Memory Impact: O(M) new connections
/// - Thread Safety: Requires exclusive access
///
/// # Panics
///
/// Panics if vector length doesn't match configured dimension
pub fn insert(&mut self, vector: Vec<f32>, id: u64) -> Result<(), HnswError> {
    // Implementation
}
```

### 3. **Type Documentation**
```rust
/// HNSW algorithm configuration parameters
///
/// This struct defines all parameters that control HNSW behavior.
/// These parameters significantly impact search quality, construction time,
/// and memory usage.
///
/// # Fields
///
/// * `dimension` - Vector dimension count (must match all vectors)
/// * `m` - Number of bi-directional links for each node (default: 16)
/// * `ef_construction` - Size of dynamic candidate list during construction (default: 200)
/// * `ml` - Maximum number of layers in the index (default: 16)
/// * `distance_metric` - Distance calculation method (default: Cosine)
///
/// # Default Configuration
///
/// The default configuration provides good performance for most use cases:
/// - Balanced search quality vs speed
/// - Reasonable memory usage (~2.5x vector size)
/// - Fast construction time
///
/// # Examples
///
/// ```rust
/// let config = HnswConfig::builder()
///     .dimension(512)
///     .m_connections(32)        // Higher M for better recall
///     .ef_construction(400)     // Higher ef for better construction
///     .distance_metric(DistanceMetric::Euclidean)
///     .build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct HnswConfig {
    /// Vector dimension count
    /// Must match all vectors inserted into the index
    pub dimension: usize,

    /// Number of connections per node (M parameter)
    /// Typical range: 5-48, higher values improve recall but increase memory
    pub m: usize,

    /// Construction ef parameter
    /// Controls dynamic candidate list size during index building
    pub ef_construction: usize,

    /// Maximum number of layers
    /// Calculated as floor(-ln(N) * ml_scale) where N is data size
    pub ml: u8,

    /// Distance metric for similarity calculation
    pub distance_metric: DistanceMetric,
}
```

---

## 🔄 **Workflow Requirements**

### 1. **Development Branch Rules**
```bash
# Create development branch
git checkout -b feature/hnsw-integration

# Work ONLY on this branch
# No commits to main/development until feature complete

# Merge only after:
# - All tests pass
# - Code review complete
# - Documentation complete
# - Performance benchmarks meet targets
```

### 2. **File Creation Sequence**
```
For each new file:
1. Create TodoWrite for the file
2. Write integration test FIRST
3. Write failing unit test
4. Implement minimal code
5. Make test pass
6. Add comprehensive documentation
7. Mark TodoWrite task as completed
8. Commit with descriptive message
```

### 3. **TodoWrite Usage Pattern**
```rust
// Before creating file
TodoWrite::todos(vec![
    TodoItem {
        content: "Create hnsw/mod.rs with public API".to_string(),
        status: "pending".to_string(),
        active_form: "Creating hnsw module structure".to_string(),
    }
]);

// When starting work
TodoWrite::todos(vec![
    TodoItem {
        content: "Create hnsw/mod.rs with public API".to_string(),
        status: "in_progress".to_string(),
        active_form: "Implementing hnsw module structure".to_string(),
    }
]);

// When complete
TodoWrite::todos(vec![
    TodoItem {
        content: "Create hnsw/mod.rs with public API".to_string(),
        status: "completed".to_string(),
        active_form: "Completed hnsw module structure".to_string(),
    }
]);
```

---

## ✅ **Compliance Checklist**

Before committing ANY code, verify:

- [ ] No mocks, stubs, or placeholders
- [ ] All code ≤ 1,000 LOC per file
- [ ] Comprehensive documentation for all public APIs
- [ ] Tests written BEFORE implementation
- [ ] Integration tests pass
- [ ] Regression tests pass
- [ ] No assumptions about external APIs
- [ ] Real data used in tests
- [ ] TodoWrite updated for file
- [ ] Single responsibility per file
- [ ] Proper error handling throughout
- [ ] Performance benchmarks passing

---

## 🚫 **Immediate Failure Conditions**

ANY violation of these rules results in:
1. Immediate stop of development
2. Code review failure
3. Required refactoring before proceeding
4. Documentation of the violation

---

**Status**: These rules are active and must be followed for ALL HNSW development work.

**Last Updated**: 2025-12-19
**Next Review**: As needed during development
