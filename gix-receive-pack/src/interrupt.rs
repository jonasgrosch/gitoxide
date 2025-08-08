//! Interrupt scaffolding for cancellation support.
//!
//! This module provides the foundation for cancellation points that will be
//! integrated in later milestones (M3/M6). When the "interrupt" feature is
//! disabled, no-op shims are provided to maintain API compatibility.

#[cfg(feature = "interrupt")]
mod enabled {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A thread-safe cancellation flag that can be shared across threads.
    ///
    /// This provides a simple mechanism for signaling cancellation to long-running
    /// operations. The flag starts in a non-cancelled state and can be set to
    /// cancelled by calling `cancel()`.
    #[derive(Debug, Default)]
    pub struct CancellationFlag(AtomicBool);

    impl CancellationFlag {
        /// Create a new cancellation flag in the non-cancelled state.
        pub fn new() -> Self {
            Self(AtomicBool::new(false))
        }

        /// Signal cancellation by setting the flag to true.
        ///
        /// This operation is atomic and thread-safe. Once cancelled, the flag
        /// cannot be reset to non-cancelled state.
        pub fn cancel(&self) {
            self.0.store(true, Ordering::Relaxed);
        }

        /// Check if cancellation has been requested.
        ///
        /// Returns `true` if `cancel()` has been called on this flag.
        pub fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    /// Trait for types that can check for cancellation and return an error if cancelled.
    ///
    /// This trait provides a standard interface for cancellation points throughout
    /// the codebase. Implementations should check their cancellation state and
    /// return `Error::Cancelled` if cancellation has been requested.
    pub trait CancellationPoint {
        /// Check for cancellation and return an error if cancelled.
        ///
        /// This method should be called at appropriate points during long-running
        /// operations to allow for graceful cancellation.
        fn check(&self) -> Result<(), crate::Error>;
    }

    /// Default implementation of CancellationPoint for CancellationFlag.
    ///
    /// This implementation checks the flag and returns `Error::Cancelled` if
    /// the flag has been set.
    impl CancellationPoint for CancellationFlag {
        fn check(&self) -> Result<(), crate::Error> {
            if self.is_cancelled() {
                Err(crate::Error::Cancelled)
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(not(feature = "interrupt"))]
mod disabled {
    /// No-op cancellation flag when interrupt feature is disabled.
    ///
    /// This provides the same API as the enabled version but with no-op
    /// implementations to maintain compatibility when the interrupt feature
    /// is not enabled.
    #[derive(Debug, Default)]
    pub struct CancellationFlag;

    impl CancellationFlag {
        /// Create a new no-op cancellation flag.
        pub fn new() -> Self {
            Self
        }

        /// No-op cancel operation.
        pub fn cancel(&self) {
            // No-op when interrupt feature is disabled
        }

        /// Always returns false when interrupt feature is disabled.
        pub fn is_cancelled(&self) -> bool {
            false
        }
    }

    /// No-op trait for cancellation points when interrupt feature is disabled.
    pub trait CancellationPoint {
        /// Always returns Ok(()) when interrupt feature is disabled.
        fn check(&self) -> Result<(), crate::Error>;
    }

    /// No-op implementation that always succeeds.
    impl CancellationPoint for CancellationFlag {
        fn check(&self) -> Result<(), crate::Error> {
            Ok(())
        }
    }
}

// Re-export the appropriate implementation based on feature flag
#[cfg(feature = "interrupt")]
pub use enabled::*;

#[cfg(not(feature = "interrupt"))]
pub use disabled::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_flag_starts_not_cancelled() {
        let flag = CancellationFlag::new();
        assert!(!flag.is_cancelled());
    }

    #[test]
    fn cancellation_flag_can_be_cancelled() {
        let flag = CancellationFlag::new();
        flag.cancel();
        
        #[cfg(feature = "interrupt")]
        assert!(flag.is_cancelled());
        
        #[cfg(not(feature = "interrupt"))]
        assert!(!flag.is_cancelled()); // No-op version always returns false
    }

    #[test]
    fn cancellation_point_check_succeeds_when_not_cancelled() {
        let flag = CancellationFlag::new();
        assert!(flag.check().is_ok());
    }

    #[test]
    fn cancellation_point_check_fails_when_cancelled() {
        let flag = CancellationFlag::new();
        flag.cancel();
        
        let result = flag.check();
        
        #[cfg(feature = "interrupt")]
        {
            assert!(result.is_err());
            if let Err(crate::Error::Cancelled) = result {
                // Expected
            } else {
                panic!("Expected Error::Cancelled, got {:?}", result);
            }
        }
        
        #[cfg(not(feature = "interrupt"))]
        assert!(result.is_ok()); // No-op version always succeeds
    }

    #[test]
    fn default_creates_non_cancelled_flag() {
        let flag = CancellationFlag::default();
        assert!(!flag.is_cancelled());
    }
}