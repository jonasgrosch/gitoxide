//! gix-serve-core: Shared server-side protocol primitives for gitoxide services.
//!
//! This crate provides minimal, reusable building blocks used by both
//! `gix-upload-pack` and `gix-receive-pack`, and the `gix-serve` orchestrator.
//!
#![deny(missing_docs, rust_2018_idioms)]
#![forbid(unsafe_code)]

#[cfg(all(feature = "blocking-io", feature = "async-io"))]
compile_error!("Cannot enable both 'blocking-io' and 'async-io' features for gix-serve-core");

pub mod service;
pub mod protocol;
pub mod visibility;
pub mod advertise;
pub mod capabilities;
pub mod pktline;
#[cfg(feature = "progress")]
pub mod progress;

// IO helpers are feature-gated to match the selected I/O mode.
#[cfg(feature = "blocking-io")]
pub mod io_blocking;
#[cfg(feature = "async-io")]
pub mod io_async;


