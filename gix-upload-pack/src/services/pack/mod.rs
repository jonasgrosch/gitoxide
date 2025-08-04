//! Pack generation and progress reporting
//!
//! This module contains all functionality related to pack file generation,
//! streaming, and progress reporting during upload-pack operations.

pub mod generation;
pub mod progress;

// Re-export commonly used types
pub use generation::{PackGenerator, PackStats};
pub use progress::ProgressReporter;