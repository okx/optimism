use alloc::vec::Vec;
use alloy_consensus::{
    Sealed, SignableTransaction, Signed, TxEip1559, TxEip4844, TypedTransaction,
};
use alloy_eips::eip7702::SignedAuthorization;
use alloy_network_primitives::TransactionBuilder7702;
use alloy_primitives::{Address, Bytes, Signature, TxKind, U256};
#[cfg(feature = "std")]
use alloy_primitives::ChainId;
use alloy_rpc_types_eth::{AccessList, TransactionInput, TransactionRequest};
use op_alloy_consensus::{
    OpTxEnvelope, OpTypedTransaction, POST_EXEC_TX_TYPE_ID, TxDeposit, TxEip8130, TxPostExec,
    transaction::eip8130::{AA_TX_TYPE_ID, AccountChangeEntry, Call},
};
use serde::{Deserialize, Serialize};

/// Builder for [`OpTypedTransaction`].
///
/// Extends the standard [`TransactionRequest`] with EIP-8130 AA fields so that
/// `eth_estimateGas` / `eth_call` can drive AA execution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpTransactionRequest {
    /// Standard Ethereum transaction request fields.
    #[serde(flatten)]
    inner: TransactionRequest,

    /// EIP-8130 2D nonce key (uint256).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "nonceKey")]
    pub nonce_key: Option<U256>,

    /// Block timestamp after which this transaction is invalid. `0` = no expiry.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub expiry: Option<u64>,

    /// EIP-8130 payer address. `Address::ZERO` means self-pay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<Address>,

    /// EIP-8130 sender authentication data.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "senderAuth")]
    pub sender_auth: Option<Bytes>,

    /// EIP-8130 payer authentication data.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "payerAuth")]
    pub payer_auth: Option<Bytes>,

    /// EIP-8130 phased call batches. Each inner `Vec` is one atomic phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<Vec<Vec<Call>>>,

    /// EIP-8130 account creation and configuration change entries.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "accountChanges")]
    pub account_changes: Option<Vec<AccountChangeEntry>>,
}

impl OpTransactionRequest {
    /// Creates a new request wrapping the given [`TransactionRequest`].
    pub fn new(inner: TransactionRequest) -> Self {
        Self { inner, ..Default::default() }
    }

    /// Returns `true` if this request targets an EIP-8130 AA transaction.
    pub fn is_eip8130(&self) -> bool {
        self.inner.transaction_type == Some(AA_TX_TYPE_ID)
            || self.calls.is_some()
            || self.account_changes.is_some()
    }

    /// Attempts to build a [`TxEip8130`] from the AA-specific fields.
    pub fn build_eip8130(&self) -> Option<TxEip8130> {
        if !self.is_eip8130() {
            return None;
        }
        Some(TxEip8130 {
            chain_id: self.inner.chain_id.unwrap_or_default(),
            from: self.inner.from,
            nonce_key: self.nonce_key.unwrap_or_default(),
            nonce_sequence: self.inner.nonce.unwrap_or_default(),
            expiry: self.expiry.unwrap_or_default(),
            max_priority_fee_per_gas: self.inner.max_priority_fee_per_gas.unwrap_or_default(),
            max_fee_per_gas: self.inner.max_fee_per_gas.unwrap_or_default(),
            gas_limit: self.inner.gas.unwrap_or_default(),
            account_changes: self.account_changes.clone().unwrap_or_default(),
            calls: self.calls.clone().unwrap_or_default(),
            payer: self.payer,
            sender_auth: self.sender_auth.clone().unwrap_or_default(),
            payer_auth: self.payer_auth.clone().unwrap_or_default(),
        })
    }

    /// Sets the `from` field in the call to the provided address
    #[inline]
    pub const fn from(mut self, from: Address) -> Self {
        self.inner.from = Some(from);
        self
    }

    /// Sets the transactions type for the transactions.
    #[doc(alias = "tx_type")]
    pub const fn transaction_type(mut self, transaction_type: u8) -> Self {
        self.inner.transaction_type = Some(transaction_type);
        self
    }

    /// Sets the gas limit for the transaction.
    pub const fn gas_limit(mut self, gas_limit: u64) -> Self {
        self.inner.gas = Some(gas_limit);
        self
    }

    /// Sets the nonce for the transaction.
    pub const fn nonce(mut self, nonce: u64) -> Self {
        self.inner.nonce = Some(nonce);
        self
    }

    /// Sets the maximum fee per gas for the transaction.
    pub const fn max_fee_per_gas(mut self, max_fee_per_gas: u128) -> Self {
        self.inner.max_fee_per_gas = Some(max_fee_per_gas);
        self
    }

    /// Sets the maximum priority fee per gas for the transaction.
    pub const fn max_priority_fee_per_gas(mut self, max_priority_fee_per_gas: u128) -> Self {
        self.inner.max_priority_fee_per_gas = Some(max_priority_fee_per_gas);
        self
    }

    /// Sets the recipient address for the transaction.
    #[inline]
    pub const fn to(mut self, to: Address) -> Self {
        self.inner.to = Some(TxKind::Call(to));
        self
    }

    /// Sets the value (amount) for the transaction.
    pub const fn value(mut self, value: U256) -> Self {
        self.inner.value = Some(value);
        self
    }

    /// Sets the chain ID for the transaction.
    pub const fn chain_id(mut self, chain_id: ChainId) -> Self {
        self.inner.chain_id = Some(chain_id);
        self
    }

    /// Sets the input data as deploy (CREATE) bytecode.
    pub fn deploy_code(mut self, code: impl Into<Bytes>) -> Self {
        self.inner.to = Some(TxKind::Create);
        self.inner.input.input = Some(code.into());
        self
    }

    /// Sets the access list for the transaction.
    pub fn access_list(mut self, access_list: AccessList) -> Self {
        self.inner.access_list = Some(access_list);
        self
    }

    /// Sets the input data for the transaction.
    pub fn input(mut self, input: TransactionInput) -> Self {
        self.inner.input = input;
        self
    }

    /// Builds [`OpTypedTransaction`] from this builder. See [`TransactionRequest::build_typed_tx`]
    /// for more info.
    ///
    /// Note that EIP-4844 transactions are not supported by Optimism and will be converted into
    /// EIP-1559 transactions.
    #[allow(clippy::result_large_err)]
    pub fn build_typed_tx(self) -> Result<OpTypedTransaction, Self> {
        if let Some(tx) = self.build_eip8130() {
            return Ok(OpTypedTransaction::Eip8130(tx));
        }

        let Self { inner, .. } = self;
        let tx = inner.build_typed_tx().map_err(|inner| Self { inner, ..Default::default() })?;
        match tx {
            TypedTransaction::Legacy(tx) => Ok(OpTypedTransaction::Legacy(tx)),
            TypedTransaction::Eip1559(tx) => Ok(OpTypedTransaction::Eip1559(tx)),
            TypedTransaction::Eip2930(tx) => Ok(OpTypedTransaction::Eip2930(tx)),
            TypedTransaction::Eip4844(tx) => {
                let tx: TxEip4844 = tx.into();
                Ok(OpTypedTransaction::Eip1559(TxEip1559 {
                    chain_id: tx.chain_id,
                    nonce: tx.nonce,
                    gas_limit: tx.gas_limit,
                    max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
                    max_fee_per_gas: tx.max_fee_per_gas,
                    to: TxKind::Call(tx.to),
                    value: tx.value,
                    access_list: tx.access_list,
                    input: tx.input,
                }))
            }
            TypedTransaction::Eip7702(tx) => Ok(OpTypedTransaction::Eip7702(tx)),
        }
    }
}

impl AsRef<TransactionRequest> for OpTransactionRequest {
    fn as_ref(&self) -> &TransactionRequest {
        &self.inner
    }
}

impl AsMut<TransactionRequest> for OpTransactionRequest {
    fn as_mut(&mut self) -> &mut TransactionRequest {
        &mut self.inner
    }
}

impl From<TransactionRequest> for OpTransactionRequest {
    fn from(inner: TransactionRequest) -> Self {
        Self::new(inner)
    }
}

impl From<OpTransactionRequest> for TransactionRequest {
    fn from(value: OpTransactionRequest) -> Self {
        value.inner
    }
}

impl From<TxDeposit> for OpTransactionRequest {
    fn from(tx: TxDeposit) -> Self {
        let TxDeposit {
            source_hash: _,
            from,
            to,
            mint: _,
            value,
            gas_limit,
            is_system_transaction: _,
            input,
        } = tx;

        Self::new(TransactionRequest {
            from: Some(from),
            to: Some(to),
            value: Some(value),
            gas: Some(gas_limit),
            input: input.into(),
            ..Default::default()
        })
    }
}

impl From<Sealed<TxDeposit>> for OpTransactionRequest {
    fn from(value: Sealed<TxDeposit>) -> Self {
        value.into_inner().into()
    }
}

impl From<TxPostExec> for OpTransactionRequest {
    fn from(tx: TxPostExec) -> Self {
        Self::new(TransactionRequest {
            from: Some(tx.signer_address()),
            transaction_type: Some(POST_EXEC_TX_TYPE_ID),
            gas: Some(0),
            nonce: Some(0),
            value: Some(U256::ZERO),
            input: tx.input.into(),
            ..Default::default()
        })
    }
}

impl From<TxEip8130> for OpTransactionRequest {
    fn from(tx: TxEip8130) -> Self {
        let from = tx.from;
        let nonce_key = Some(tx.nonce_key);
        let expiry = Some(tx.expiry);
        let payer = tx.payer;
        let sender_auth = Some(tx.sender_auth.clone());
        let payer_auth = Some(tx.payer_auth.clone());
        let calls = Some(tx.calls.clone());
        let account_changes = Some(tx.account_changes.clone());
        let mut inner = TransactionRequest::from_transaction(tx);
        inner.from = from;
        Self { inner, nonce_key, expiry, payer, sender_auth, payer_auth, calls, account_changes }
    }
}

impl From<Sealed<TxEip8130>> for OpTransactionRequest {
    fn from(value: Sealed<TxEip8130>) -> Self {
        value.into_inner().into()
    }
}

impl<T> From<Signed<T, Signature>> for OpTransactionRequest
where
    T: SignableTransaction<Signature> + Into<TransactionRequest>,
{
    fn from(value: Signed<T, Signature>) -> Self {
        #[cfg(feature = "k256")]
        let from = value.recover_signer().ok();
        #[cfg(not(feature = "k256"))]
        let from = None;

        let mut inner: TransactionRequest = value.strip_signature().into();
        inner.from = from;

        Self::new(inner)
    }
}

impl From<OpTypedTransaction> for OpTransactionRequest {
    fn from(tx: OpTypedTransaction) -> Self {
        match tx {
            OpTypedTransaction::Legacy(tx) => Self::new(tx.into()),
            OpTypedTransaction::Eip2930(tx) => Self::new(tx.into()),
            OpTypedTransaction::Eip1559(tx) => Self::new(tx.into()),
            OpTypedTransaction::Eip7702(tx) => Self::new(tx.into()),
            OpTypedTransaction::Eip8130(tx) => tx.into(),
            OpTypedTransaction::Deposit(tx) => tx.into(),
            OpTypedTransaction::PostExec(tx) => tx.into(),
        }
    }
}

impl From<OpTxEnvelope> for OpTransactionRequest {
    fn from(value: OpTxEnvelope) -> Self {
        match value {
            OpTxEnvelope::Legacy(tx) => tx.into(),
            OpTxEnvelope::Eip2930(tx) => tx.into(),
            OpTxEnvelope::Eip1559(tx) => tx.into(),
            OpTxEnvelope::Eip7702(tx) => tx.into(),
            OpTxEnvelope::Eip8130(tx) => tx.into(),
            OpTxEnvelope::Deposit(tx) => tx.into(),
            OpTxEnvelope::PostExec(tx) => tx.into_inner().into(),
        }
    }
}

impl From<super::Transaction> for OpTransactionRequest {
    fn from(tx: super::Transaction) -> Self {
        let recovered = tx.inner.into_recovered();
        let from = recovered.signer();
        let mut req: Self = recovered.into_inner().into();
        req.inner.from = Some(from);
        req
    }
}

impl TransactionBuilder7702 for OpTransactionRequest {
    fn authorization_list(&self) -> Option<&Vec<SignedAuthorization>> {
        self.as_ref().authorization_list()
    }

    fn set_authorization_list(&mut self, authorization_list: Vec<SignedAuthorization>) {
        self.as_mut().set_authorization_list(authorization_list);
    }
}

#[cfg(feature = "std")]
impl alloy_network::TransactionBuilder for OpTransactionRequest {
    fn chain_id(&self) -> Option<ChainId> {
        self.as_ref().chain_id()
    }

    fn set_chain_id(&mut self, chain_id: ChainId) {
        self.as_mut().set_chain_id(chain_id);
    }

    fn nonce(&self) -> Option<u64> {
        self.as_ref().nonce()
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.as_mut().set_nonce(nonce);
    }

    fn take_nonce(&mut self) -> Option<u64> {
        self.as_mut().nonce.take()
    }

    fn input(&self) -> Option<&Bytes> {
        self.as_ref().input()
    }

    fn set_input<T: Into<Bytes>>(&mut self, input: T) {
        self.as_mut().set_input(input);
    }

    fn from(&self) -> Option<Address> {
        self.as_ref().from()
    }

    fn set_from(&mut self, from: Address) {
        self.as_mut().set_from(from);
    }

    fn kind(&self) -> Option<TxKind> {
        self.as_ref().kind()
    }

    fn clear_kind(&mut self) {
        self.as_mut().clear_kind();
    }

    fn set_kind(&mut self, kind: TxKind) {
        self.as_mut().set_kind(kind);
    }

    fn value(&self) -> Option<U256> {
        self.as_ref().value()
    }

    fn set_value(&mut self, value: U256) {
        self.as_mut().set_value(value);
    }

    fn gas_price(&self) -> Option<u128> {
        self.as_ref().gas_price()
    }

    fn set_gas_price(&mut self, gas_price: u128) {
        self.as_mut().set_gas_price(gas_price);
    }

    fn max_fee_per_gas(&self) -> Option<u128> {
        self.as_ref().max_fee_per_gas()
    }

    fn set_max_fee_per_gas(&mut self, max_fee_per_gas: u128) {
        self.as_mut().set_max_fee_per_gas(max_fee_per_gas);
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.as_ref().max_priority_fee_per_gas()
    }

    fn set_max_priority_fee_per_gas(&mut self, max_priority_fee_per_gas: u128) {
        self.as_mut().set_max_priority_fee_per_gas(max_priority_fee_per_gas);
    }

    fn gas_limit(&self) -> Option<u64> {
        self.as_ref().gas_limit()
    }

    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.as_mut().set_gas_limit(gas_limit);
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.as_ref().access_list()
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.as_mut().set_access_list(access_list);
    }
}
