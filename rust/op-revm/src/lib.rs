//! Optimism-specific constants, types, and helpers.
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc as std;
#[cfg(feature = "std")]
extern crate std;

// `alloy-primitives` is re-exported via `revm::primitives::*` and only used
// indirectly through that path, but it appears as a direct dep so that the
// crate's `std` feature can gate `alloy-primitives/std`.
use alloy_primitives as _;

pub mod api;
pub mod constants;
pub mod eip8130_gas;
pub mod eip8130_policy;
pub mod evm;
pub mod fast_lz;
pub mod gas_params;
pub mod handler;
pub mod l1block;
pub mod precompiles;
pub mod precompiles_xlayer;
pub mod result;
pub mod spec;
pub mod transaction;

pub use revm;

pub use api::{
    builder::OpBuilder,
    default_ctx::{DefaultOp, OpContext},
};
pub use evm::OpEvm;
pub use l1block::L1BlockInfo;
pub use result::OpHaltReason;
pub use spec::*;
pub use transaction::{
    OpEip8130TxTr, OpTransaction, OpTransactionError, OpTxTr, estimate_tx_compressed_size,
};
