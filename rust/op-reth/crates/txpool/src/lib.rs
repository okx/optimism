//! OP-Reth Transaction pool.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod validator;
pub use validator::{OpL1BlockInfo, OpTransactionValidator};

pub mod aa_validator;
pub use aa_validator::OpAaTransactionValidator;

pub mod best;
pub mod conditional;
pub mod dual_pool;
pub mod eip8130_pool;
pub mod eip8130_xlayer;
pub use best::{BestAaTransactions, MergeBestTransactions};
pub use dual_pool::OpDualPool;
pub use eip8130_pool::{
    Eip8130AddOutcome, Eip8130PendingAdded, Eip8130Pool, Eip8130PoolConfig, Eip8130PoolTx,
    Eip8130SeqId, Eip8130StateUpdateOutcome, Eip8130TxId,
};
pub use eip8130_xlayer::{
    Eip8130ValidationError, MAX_AA_TX_ENCODED_BYTES, validate_eip8130_transaction,
};
mod pool;
pub use pool::OpPool;
pub mod supervisor;
mod transaction;
pub use transaction::{OpPooledTransaction, OpPooledTx};
mod error;
pub mod interop;
pub mod maintain;
pub use error::InvalidCrossTx;
pub mod estimated_da_size;

use reth_transaction_pool::{CoinbaseTipOrdering, Pool, TransactionValidationTaskExecutor};

/// Type alias for default optimism transaction pool.
///
/// The [`OpPool`] wrapper delegates most behavior to the inner [`Pool`] handle,
/// and overrides only a subset of the functions.
/// This enables implementing custom behaviors and filtering of the pooled transactions.
pub type OpTransactionPool<Client, S, Evm, T = OpPooledTransaction> = OpPool<
    Pool<
        TransactionValidationTaskExecutor<OpTransactionValidator<Client, T, Evm>>,
        CoinbaseTipOrdering<T>,
        S,
    >,
>;

/// Optimism transaction pool augmented with the EIP-8130 (XLayer AA)
/// 2D-nonce side pool.
///
/// `OpDualPool<P, Client>` stores an `OpPool<P>` internally; the type
/// parameter `P` is the **raw** [`reth_transaction_pool::Pool`] — not
/// [`OpTransactionPool`] — to avoid a double `OpPool` wrap. `Client` is
/// the state-provider factory used during AA admission for nonce-slot
/// reads. The protocol pool is parametrised on
/// [`OpAaTransactionValidator`] (the unified wrapper) so non-AA txs and
/// any AA tx that accidentally reaches the protocol pool both run the
/// AA-spec layer in addition to reth's standard mempool gates.
pub type OpAaTransactionPool<Client, S, Evm, T = OpPooledTransaction> = OpDualPool<
    Pool<
        TransactionValidationTaskExecutor<
            OpAaTransactionValidator<OpTransactionValidator<Client, T, Evm>, Client>,
        >,
        CoinbaseTipOrdering<T>,
        S,
    >,
    Client,
    OpAaTransactionValidator<OpTransactionValidator<Client, T, Evm>, Client>,
>;
