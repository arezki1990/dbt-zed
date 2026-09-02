//! Progress tracking for long-running operations.
//!
//! This module provides callback-based progress reporting for algorithms
//! that may take significant time to execute (PageRank, betweenness centrality, etc.).
//!
//! # Key Types
//!
//! - [`ProgressCallback`] - Trait for progress reporting
//! - [`NoProgress`] - Zero-overhead no-op implementation (default)
//! - [`ConsoleProgress`] - CLI-friendly stderr output
//! - [`ProgressState`] - Throttled wrapper to avoid spam
//!
//! # Usage Patterns
//!
//! ## Zero-Overhead Progress (Default)
//!
//! Use [`NoProgress`] when you don't need progress reporting:
//!
//! ```rust,ignore
//! use sqlitegraph::{algo::pagerank, progress::NoProgress};
//!
//! let results = pagerank(&graph)?;
//! // No progress output, zero overhead
//! ```
//!
//! ## Console Progress for CLI
//!
//! Use [`ConsoleProgress`] for CLI applications:
//!
//! ```rust,ignore
//! use sqlitegraph::{algo::pagerank_with_progress, progress::ConsoleProgress};
//!
//! let progress = ConsoleProgress::new();
//! let results = pagerank_with_progress(&graph, progress)?;
//! // Output to stderr:
//! // PageRank iteration 1/100...
//! // PageRank iteration 2/100...
//! // ...
//! ```
//!
//! Note: Progress is written to **stderr** to avoid interfering with
//! data output on stdout.
//!
//! # ProgressCallback Trait
//!
//! The [`ProgressCallback`] trait defines the interface for progress reporting:
//!
//! ## Thread Safety
//!
//! All implementations must be `Send + Sync` for thread-safe use:
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use sqlitegraph::progress::ConsoleProgress;
//!
//! let progress = Arc::new(ConsoleProgress::new());
//! // Safe to share across threads
//! ```
//!
//! ## Callback Methods
//!
//! - **`on_progress(current, total, message)`**: Called repeatedly during operation
//! - **`on_complete()`**: Called exactly once on success
//! - **`on_error(error)`**: Called exactly once on failure
//!
//! # Implementations
//!
//! ## NoProgress
//!
//! Zero-overhead no-op implementation:
//!
//! - **Cost**: Zero (all methods are `#[inline]` no-ops)
//! - **Use case**: Library code, batch processing, tests
//! - **Output**: None
//!
//! ```rust,ignore
//! let progress = NoProgress;
//! progress.on_progress(50, Some(100), "Processing"); // Does nothing
//! ```
//!
//! ## ConsoleProgress
//!
//! CLI-friendly stderr output:
//!
//! - **Cost**: Minimal (formatted write to stderr)
//! - **Use case**: Interactive CLI applications
//! - **Output**: `Message [current/total]` or `Message: current`
//!
//! ```rust,ignore
//! let console = ConsoleProgress::new();
//! console.on_progress(5, Some(10), "Processing");
//! // Output: Processing [5/10]
//!
//! console.on_progress(5, None, "Processing");
//! // Output: Processing: 5
//! ```
//!
//! ## ProgressState
//!
//! Throttled wrapper to avoid spam:
//!
//! - **Cost**: Minimal (time-checked throttling)
//! - **Use case**: High-frequency progress updates
//! - **Behavior**: Only calls underlying callback every N milliseconds
//!
//! ```rust,ignore
//! use std::time::Duration;
//! use sqlitegraph::progress::ProgressState;
//!
//! let base = ConsoleProgress::new();
//! let throttled = ProgressState::new(base, Duration::from_millis(100));
//!
//! // Only outputs every 100ms, even if called more frequently
//! for i in 0..1000 {
//!     throttled.on_progress(i, Some(1000), "Processing");
//! }
//! ```
//!
//! # Progress Throttling
//!
//! High-frequency progress updates can cause performance issues and output spam.
//! [`ProgressState`] addresses this with **time-based throttling**:
//!
//! ## Throttling Behavior
//!
//! - **Minimum interval**: Configurable (default 100ms)
//! - **First call**: Always executes
//! - **Subsequent calls**: Only if `now - last_call >= min_interval`
//! - **Completion**: Always calls `on_complete()` (not throttled)
//! - **Errors**: Always calls `on_error()` (not throttled)
//!
//! ## Why Throttle?
//!
//! - **Performance**: Avoid excessive I/O from rapid updates
//! - **UX**: Prevent unreadable rapid-fire output
//! - **LLM-friendly**: Provide summarized progress for AI consumption
//!
//! # Using with Algorithms
//!
//! Progress-tracking variants are available for long-running algorithms:
//!
//! ```rust,ignore
//! use sqlitegraph::{
//!     algo::{pagerank_with_progress, louvain_communities_with_progress},
//!     progress::ConsoleProgress
//! };
//!
//! let progress = ConsoleProgress::new();
//!
//! // PageRank with progress
//! let rankings = pagerank_with_progress(&graph, progress.clone())?;
//!
//! // Louvain with progress
//! let communities = louvain_communities_with_progress(&graph, progress)?;
//! ```
//!
//! Available `_with_progress` variants:
//! - [`pagerank_with_progress`](crate::algo::pagerank_with_progress)
//! - [`betweenness_centrality_with_progress`](crate::algo::betweenness_centrality_with_progress)
//! - [`louvain_communities_with_progress`](crate::algo::louvain_communities_with_progress)
//!
//! # Custom Implementations
//!
//! Implement [`ProgressCallback`] for custom behavior:
//!
//! ```rust,ignore
//! use sqlitegraph::progress::ProgressCallback;
//!
//! struct CustomProgress {
//!     start_time: std::time::Instant,
//! }
//!
//! impl ProgressCallback for CustomProgress {
//!     fn on_progress(&self, current: usize, total: Option<usize>, message: &str) {
//!         let elapsed = self.start_time.elapsed().as_secs_f64();
//!         match total {
//!             Some(total) => {
//!                 let percent = (current as f64 / total as f64) * 100.0;
//!                 println!("{}: {:.1}% ({:.2}s elapsed)", message, percent, elapsed);
//!             }
//!             None => {
//!                 println!("{}: {} ({:.2}s elapsed)", message, current, elapsed);
//!             }
//!         }
//!     }
//!
//!     fn on_complete(&self) {
//!         let elapsed = self.start_time.elapsed().as_secs_f64();
//!         println!("Complete in {:.2}s", elapsed);
//!     }
//!
//!     fn on_error(&self, error: &dyn std::error::Error) {
//!         eprintln!("Error: {}", error);
//!     }
//! }
//! ```

use parking_lot::Mutex;
use std::time::{Duration, Instant};

/// Callback trait for progress reporting on long-running operations.
///
/// This trait allows algorithms to report progress updates during execution,
/// enabling user feedback and LLM visibility into operation status.
///
/// # Thread Safety
///
/// All methods are thread-safe (require `Send + Sync`), allowing progress
/// callbacks to be shared across threads if needed.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::progress::ProgressCallback;
///
/// struct MyCallback {
///     // Your fields here
/// }
///
/// impl ProgressCallback for MyCallback {
///     fn on_progress(&self, current: usize, total: Option<usize>, message: &str) {
///         // Handle progress update
///     }
///
///     fn on_complete(&self) {
///         // Handle completion
///     }
///
///     fn on_error(&self, error: &dyn std::error::Error) {
///         // Handle error
///     }
/// }
/// ```
pub trait ProgressCallback: Send + Sync {
    /// Called when progress is made.
    ///
    /// # Parameters
    /// - `current`: Current step or item being processed
    /// - `total`: Total number of steps (if known), `None` for indeterminate operations
    /// - `message`: Human-readable progress message
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlitegraph::progress::ProgressCallback;
    /// # struct MyCallback;
    /// # impl ProgressCallback for MyCallback {
    /// fn on_progress(&self, current: usize, total: Option<usize>, message: &str) {
    ///     match total {
    ///         Some(total) => println!("{}: {}/{}", message, current, total),
    ///         None => println!("{}: {}", message, current),
    ///     }
    /// }
    /// # fn on_complete(&self) {}
    /// # fn on_error(&self, _: &dyn std::error::Error) {}
    /// # }
    /// ```
    fn on_progress(&self, current: usize, total: Option<usize>, message: &str);

    /// Called when the operation completes successfully.
    ///
    /// This is called exactly once if no errors occur.
    fn on_complete(&self);

    /// Called when the operation encounters an error.
    ///
    /// # Parameters
    /// - `error`: The error that caused the operation to fail
    ///
    /// This is called exactly once if an error occurs, and `on_complete` will not be called.
    fn on_error(&self, error: &dyn std::error::Error);
}

/// No-op progress callback (default implementation).
///
/// This implementation does nothing, allowing progress-based APIs
/// to have zero overhead when progress reporting is not needed.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::progress::{NoProgress, ProgressCallback};
///
/// let progress = NoProgress;
/// progress.on_progress(5, Some(10), "Processing..."); // Does nothing
/// progress.on_complete(); // Does nothing
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProgress;

impl ProgressCallback for NoProgress {
    #[inline]
    fn on_progress(&self, _current: usize, _total: Option<usize>, _message: &str) {
        // No-op
    }

    #[inline]
    fn on_complete(&self) {
        // No-op
    }

    #[inline]
    fn on_error(&self, _error: &dyn std::error::Error) {
        // No-op
    }
}

/// Console progress reporter for CLI use.
///
/// Prints progress updates to stderr, making it suitable for CLI applications
/// where stdout may be used for data output.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::progress::{ConsoleProgress, ProgressCallback};
///
/// let console = ConsoleProgress::new();
/// console.on_progress(5, Some(10), "Processing");
/// // Output to stderr: Processing [5/10]
/// ```
#[derive(Debug)]
pub struct ConsoleProgress {
    // Not strictly needed for Mutex, but provides future flexibility
    // for potential shared state across threads
    _private: (),
}

impl ConsoleProgress {
    /// Creates a new console progress reporter.
    ///
    /// # Example
    ///
    /// ```rust
    /// use sqlitegraph::progress::ConsoleProgress;
    ///
    /// let console = ConsoleProgress::new();
    /// ```
    #[inline]
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for ConsoleProgress {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressCallback for ConsoleProgress {
    fn on_progress(&self, current: usize, total: Option<usize>, message: &str) {
        match total {
            Some(total) => eprintln!("{} [{}/{}]", message, current, total),
            None => eprintln!("{} [{}]", message, current),
        }
    }

    fn on_complete(&self) {
        eprintln!("Complete");
    }

    fn on_error(&self, error: &dyn std::error::Error) {
        eprintln!("Error: {}", error);
    }
}

/// Helper wrapper for throttling progress callback frequency.
///
/// Some operations progress very quickly (e.g., processing thousands of items),
/// and reporting progress on every item would overwhelm the callback and impact
/// performance. This wrapper enforces a minimum time between updates.
///
/// # Example
///
/// ```rust
/// use sqlitegraph::progress::{ProgressCallback, ProgressState, NoProgress};
/// use std::time::Duration;
///
/// let inner = NoProgress;
/// let mut progress = ProgressState::new(&inner, Duration::from_millis(100));
///
/// // Only reports if at least 100ms has passed since last report
/// progress.update(5, Some(10), "Processing");
/// progress.update(6, Some(10), "Processing"); // May be skipped
/// ```
#[derive(Debug)]
pub struct ProgressState<'a, F>
where
    F: ProgressCallback + ?Sized,
{
    callback: &'a F,
    interval: Duration,
    last_update: Mutex<Instant>,
}

impl<'a, F> ProgressState<'a, F>
where
    F: ProgressCallback + ?Sized,
{
    /// Creates a new progress state wrapper.
    ///
    /// # Parameters
    /// - `callback`: The underlying progress callback to wrap
    /// - `interval`: Minimum time between progress updates
    ///
    /// # Example
    ///
    /// ```rust
    /// use sqlitegraph::progress::{ProgressCallback, ProgressState, NoProgress};
    /// use std::time::Duration;
    ///
    /// let callback = NoProgress;
    /// let progress = ProgressState::new(&callback, Duration::from_millis(100));
    /// ```
    #[inline]
    pub fn new(callback: &'a F, interval: Duration) -> Self {
        Self {
            callback,
            interval,
            last_update: Mutex::new(Instant::now() - interval), // Allow immediate first update
        }
    }

    /// Updates progress, but only if the minimum interval has elapsed.
    ///
    /// # Parameters
    /// - `current`: Current step or item being processed
    /// - `total`: Total number of steps (if known)
    /// - `message`: Human-readable progress message
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlitegraph::progress::{ProgressCallback, ProgressState, NoProgress};
    /// # use std::time::Duration;
    /// # let callback = NoProgress;
    /// # let mut progress = ProgressState::new(&callback, Duration::from_millis(100));
    /// // Returns immediately if interval hasn't elapsed
    /// progress.update(50, Some(100), "Processing");
    /// ```
    pub fn update(&mut self, current: usize, total: Option<usize>, message: &str) {
        let mut last_update = self.last_update.lock();

        let now = Instant::now();

        if now.duration_since(*last_update) >= self.interval {
            self.callback.on_progress(current, total, message);
            *last_update = now;
        }
    }

    /// Forces an immediate progress update, bypassing the throttling logic.
    ///
    /// Use this for important milestones (e.g., completion) that should
    /// always be reported regardless of timing.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlitegraph::progress::{ProgressCallback, ProgressState, NoProgress};
    /// # use std::time::Duration;
    /// # let callback = NoProgress;
    /// # let mut progress = ProgressState::new(&callback, Duration::from_secs(10));
    /// // Always report the final update, even if interval hasn't elapsed
    /// progress.force_update(100, Some(100), "Complete");
    /// ```
    #[inline]
    pub fn force_update(&mut self, current: usize, total: Option<usize>, message: &str) {
        self.callback.on_progress(current, total, message);
        *self.last_update.lock() = Instant::now();
    }

    /// Returns the configured update interval.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlitegraph::progress::{ProgressState, NoProgress};
    /// # use std::time::Duration;
    /// # let callback = NoProgress;
    /// # let progress = ProgressState::new(&callback, Duration::from_millis(100));
    /// let interval = progress.update_interval();
    /// assert_eq!(interval, Duration::from_millis(100));
    /// ```
    #[inline]
    pub fn update_interval(&self) -> Duration {
        self.interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct TestCallback {
        progress_count: AtomicUsize,
        complete_count: AtomicUsize,
        error_count: AtomicUsize,
    }

    impl TestCallback {
        fn new() -> Self {
            Self {
                progress_count: AtomicUsize::new(0),
                complete_count: AtomicUsize::new(0),
                error_count: AtomicUsize::new(0),
            }
        }

        fn progress_count(&self) -> usize {
            self.progress_count.load(Ordering::SeqCst)
        }

        fn complete_count(&self) -> usize {
            self.complete_count.load(Ordering::SeqCst)
        }

        fn error_count(&self) -> usize {
            self.error_count.load(Ordering::SeqCst)
        }
    }

    impl ProgressCallback for TestCallback {
        fn on_progress(&self, _current: usize, _total: Option<usize>, _message: &str) {
            self.progress_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_complete(&self) {
            self.complete_count.fetch_add(1, Ordering::SeqCst);
        }

        fn on_error(&self, _error: &dyn std::error::Error) {
            self.error_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_no_progress_is_no_op() {
        let progress = NoProgress;
        // Should not panic or do anything
        progress.on_progress(5, Some(10), "Test");
        progress.on_complete();
        progress.on_error(&std::io::Error::other("test"));
    }

    #[test]
    fn test_callback_invocation() {
        let callback = TestCallback::new();

        assert_eq!(callback.progress_count(), 0);
        assert_eq!(callback.complete_count(), 0);
        assert_eq!(callback.error_count(), 0);

        callback.on_progress(1, Some(10), "Test 1");
        callback.on_progress(2, Some(10), "Test 2");
        callback.on_complete();

        assert_eq!(callback.progress_count(), 2);
        assert_eq!(callback.complete_count(), 1);
        assert_eq!(callback.error_count(), 0);
    }

    #[test]
    fn test_error_invocation() {
        let callback = TestCallback::new();

        let error = std::io::Error::other("test error");
        callback.on_error(&error);

        assert_eq!(callback.progress_count(), 0);
        assert_eq!(callback.complete_count(), 0);
        assert_eq!(callback.error_count(), 1);
    }

    #[test]
    fn test_progress_state_throttling() {
        let callback = TestCallback::new();
        let interval = Duration::from_millis(50);
        let mut progress = ProgressState::new(&callback, interval);

        // First update should always succeed (last_update is initialized in the past)
        progress.update(1, Some(10), "Test 1");
        assert_eq!(callback.progress_count(), 1);

        // Immediate second update should be throttled
        progress.update(2, Some(10), "Test 2");
        assert_eq!(callback.progress_count(), 1); // Still 1

        // Force update should bypass throttling
        progress.force_update(3, Some(10), "Test 3");
        assert_eq!(callback.progress_count(), 2); // Now 2

        // Wait for interval to elapse
        std::thread::sleep(interval);

        // Next update should succeed
        progress.update(4, Some(10), "Test 4");
        assert_eq!(callback.progress_count(), 3); // Now 3
    }

    #[test]
    fn test_progress_state_update_interval() {
        let callback = NoProgress;
        let interval = Duration::from_millis(100);
        let progress = ProgressState::new(&callback, interval);

        assert_eq!(progress.update_interval(), interval);
    }

    #[test]
    fn test_console_progress_default() {
        let console = ConsoleProgress::default();
        // Just verify it compiles and doesn't panic
        console.on_progress(5, Some(10), "Test");
        console.on_complete();
    }
}
