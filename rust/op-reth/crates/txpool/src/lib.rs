//! OP-Reth Transaction pool.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod eip8130_invalidation;
pub use eip8130_invalidation::{
    Eip8130InvalidationIndex, InvalidationKey, compute_invalidation_keys,
    maintain_eip8130_invalidation, process_fal,
};

mod eip8130_pool;
pub use eip8130_pool::{
    AddOutcome, BestEip8130Transactions, Eip8130Pool, Eip8130PoolConfig, Eip8130PoolError,
    Eip8130SequenceId, Eip8130TxId, SharedEip8130Pool, ThroughputTier, TierCheckResult,
};

mod base_pool;
pub use base_pool::BaseTransactionPool;

mod best;
pub use best::MergedBestTransactions;

mod eip8130_validate;
pub use eip8130_validate::{
    CustomVerifierPolicy, DEFAULT_CUSTOM_VERIFIER_GAS_LIMIT, Eip8130ValidationError,
    Eip8130ValidationOutcome, MAX_AA_TX_ENCODED_BYTES, VerifierAdmissionPolicy, VerifierAllowlist,
    VerifierPurityCache, compute_account_tier, validate_eip8130_transaction,
};

mod validator;
pub use validator::{OpL1BlockInfo, OpTransactionValidator};

pub mod conditional;
mod pool;
pub use pool::OpPool;
pub mod supervisor;
mod transaction;
pub use transaction::{Eip8130Metadata, OpPooledTransaction, OpPooledTx};
mod error;
pub mod interop;
pub mod maintain;
pub use error::InvalidCrossTx;
pub mod estimated_da_size;

use reth_transaction_pool::{CoinbaseTipOrdering, Pool, TransactionValidationTaskExecutor};

/// Type alias for default optimism transaction pool.
///
/// Layering:
/// - [`Pool`] — reth's standard pool, owns the validator (which itself holds
///   the EIP-8130 side-pool).
/// - [`BaseTransactionPool`] — fans out reads/writes to both the standard pool
///   and the EIP-8130 side-pool so the rest of the node sees a single pool
///   handle that exposes AA transactions alongside everything else.
/// - [`OpPool`] — adds OP-specific interop reorg filtering on top.
pub type OpTransactionPool<Client, S, Evm, T = OpPooledTransaction> = OpPool<
    BaseTransactionPool<
        Pool<
            TransactionValidationTaskExecutor<OpTransactionValidator<Client, T, Evm>>,
            CoinbaseTipOrdering<T>,
            S,
        >,
        T,
    >,
>;
