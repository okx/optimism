//! Standalone crate for Optimism-specific Reth configuration and builder types.
//!
//! # features
//! - `js-tracer`: Enable the `JavaScript` tracer for the `debug_trace` endpoints

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
// Adding the EIP-8130 variant to `op_alloy_consensus::OpPooledTransaction` widened the
// transitive type-graph used by `BasicPayloadJob<...>`'s `Unpin` auto-trait check past
// rustc 1.94's default `recursion_limit = 128`, manifesting as an `is_unpin_raw` ICE in
// the trait solver. The obligation itself terminates fine — auto-traits stop at concrete
// types — but the depth of the type-tree walk needs more headroom. 256 is well within the
// "honest depth" range and matches how alloy/reth handle similar deeply-generic crates.
#![recursion_limit = "256"]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[cfg(feature = "test-utils")]
use reth_db_api as _;

/// CLI argument parsing for the optimism node.
pub mod args;

/// Exports optimism-specific implementations of the [`EngineTypes`](reth_node_api::EngineTypes)
/// trait.
pub mod engine;
pub use engine::OpEngineTypes;

pub mod node;
pub use node::*;

pub mod rpc;
pub use rpc::OpEngineApiBuilder;

pub mod version;
pub use version::OP_NAME_CLIENT;

pub use reth_optimism_txpool as txpool;

pub mod proof_history;

/// Helpers for running test node instances.
#[cfg(feature = "test-utils")]
pub mod utils;

pub use reth_optimism_payload_builder::{
    self as payload, OpBuiltPayload, OpPayloadAttributes, OpPayloadBuilder,
    OpPayloadBuilderAttributes, OpPayloadPrimitives, OpPayloadTypes, config::OpDAConfig,
};

pub use reth_optimism_evm::*;

pub use reth_optimism_storage::OpStorage;

use op_revm as _;
use revm as _;

#[cfg(feature = "test-utils")]
use reth_tasks as _;
