//! In-memory block buffer and overlay state provider for the engine.
//!
//! `TrieBuffer` holds blocks that have been processed but not yet flushed to disk.
//! `MemoryOverlayOpProofsStateProviderRef` layers that buffer on top of persistent
//! storage so that block execution can read from the full chain view.

#[cfg(feature = "metrics")]
mod metrics;
pub mod overlay;
pub mod state;

pub use state::TrieBufferState;
