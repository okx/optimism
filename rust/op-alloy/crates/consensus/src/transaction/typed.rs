pub use crate::transaction::envelope::OpTypedTransaction;
use crate::{OpTxEnvelope, OpTxType, TxDeposit, eip8130::TxEip8130};
use alloy_consensus::{
    EthereumTypedTransaction, Sealable, SignableTransaction, Signed, TxEip1559, TxEip2930,
    TxEip7702, TxLegacy, Typed2718, TypedTransaction, error::ValueError,
    transaction::RlpEcdsaEncodableTx,
};
use alloy_eips::Encodable2718;
use alloy_primitives::{B256, ChainId, Signature, TxHash, bytes::BufMut};
use alloy_rlp::Encodable;

impl From<TxLegacy> for OpTypedTransaction {
    fn from(tx: TxLegacy) -> Self {
        Self::Legacy(tx)
    }
}

impl From<TxEip2930> for OpTypedTransaction {
    fn from(tx: TxEip2930) -> Self {
        Self::Eip2930(tx)
    }
}

impl From<TxEip1559> for OpTypedTransaction {
    fn from(tx: TxEip1559) -> Self {
        Self::Eip1559(tx)
    }
}

impl From<TxEip7702> for OpTypedTransaction {
    fn from(tx: TxEip7702) -> Self {
        Self::Eip7702(tx)
    }
}

impl From<TxDeposit> for OpTypedTransaction {
    fn from(tx: TxDeposit) -> Self {
        Self::Deposit(tx)
    }
}

impl From<TxEip8130> for OpTypedTransaction {
    fn from(tx: TxEip8130) -> Self {
        Self::Eip8130(tx)
    }
}

impl From<OpTxEnvelope> for OpTypedTransaction {
    fn from(envelope: OpTxEnvelope) -> Self {
        match envelope {
            OpTxEnvelope::Legacy(tx) => Self::Legacy(tx.strip_signature()),
            OpTxEnvelope::Eip2930(tx) => Self::Eip2930(tx.strip_signature()),
            OpTxEnvelope::Eip1559(tx) => Self::Eip1559(tx.strip_signature()),
            OpTxEnvelope::Eip7702(tx) => Self::Eip7702(tx.strip_signature()),
            OpTxEnvelope::Eip8130(tx) => Self::Eip8130(tx.into_inner()),
            OpTxEnvelope::Deposit(tx) => Self::Deposit(tx.into_inner()),
        }
    }
}

impl<Eip4844> TryFrom<OpTypedTransaction> for EthereumTypedTransaction<Eip4844> {
    type Error = ValueError<OpTypedTransaction>;

    fn try_from(value: OpTypedTransaction) -> Result<Self, Self::Error> {
        value.try_into_eth_variant()
    }
}

#[cfg(feature = "alloy-compat")]
impl From<OpTypedTransaction> for alloy_rpc_types_eth::TransactionRequest {
    fn from(tx: OpTypedTransaction) -> Self {
        match tx {
            OpTypedTransaction::Legacy(tx) => tx.into(),
            OpTypedTransaction::Eip2930(tx) => tx.into(),
            OpTypedTransaction::Eip1559(tx) => tx.into(),
            OpTypedTransaction::Eip7702(tx) => tx.into(),
            // See envelope.rs for why Eip8130 projects to default.
            OpTypedTransaction::Eip8130(_) => Self::default(),
            OpTypedTransaction::Deposit(tx) => tx.into(),
        }
    }
}

impl OpTypedTransaction {
    /// Return the [`OpTxType`] of the inner txn.
    pub const fn tx_type(&self) -> OpTxType {
        match self {
            Self::Legacy(_) => OpTxType::Legacy,
            Self::Eip2930(_) => OpTxType::Eip2930,
            Self::Eip1559(_) => OpTxType::Eip1559,
            Self::Eip7702(_) => OpTxType::Eip7702,
            Self::Eip8130(_) => OpTxType::Eip8130,
            Self::Deposit(_) => OpTxType::Deposit,
        }
    }

    /// Calculates the signing hash for the transaction.
    ///
    /// Returns `None` for transaction types that don't use an
    /// external ECDSA signature ([`Self::Deposit`], [`Self::Eip8130`]).
    pub fn checked_signature_hash(&self) -> Option<B256> {
        match self {
            Self::Legacy(tx) => Some(tx.signature_hash()),
            Self::Eip2930(tx) => Some(tx.signature_hash()),
            Self::Eip1559(tx) => Some(tx.signature_hash()),
            Self::Eip7702(tx) => Some(tx.signature_hash()),
            Self::Eip8130(_) | Self::Deposit(_) => None,
        }
    }

    /// Return the inner legacy transaction if it exists.
    pub const fn legacy(&self) -> Option<&TxLegacy> {
        match self {
            Self::Legacy(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner EIP-2930 transaction if it exists.
    pub const fn eip2930(&self) -> Option<&TxEip2930> {
        match self {
            Self::Eip2930(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner EIP-1559 transaction if it exists.
    pub const fn eip1559(&self) -> Option<&TxEip1559> {
        match self {
            Self::Eip1559(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner deposit transaction if it exists.
    pub const fn deposit(&self) -> Option<&TxDeposit> {
        match self {
            Self::Deposit(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns `true` if transaction is deposit transaction.
    pub const fn is_deposit(&self) -> bool {
        matches!(self, Self::Deposit(_))
    }

    /// Calculate the transaction hash for the given signature.
    ///
    /// Note: Returns the regular tx hash if this is a deposit variant
    pub fn tx_hash(&self, signature: &Signature) -> TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash(signature),
            Self::Eip2930(tx) => tx.tx_hash(signature),
            Self::Eip1559(tx) => tx.tx_hash(signature),
            Self::Eip7702(tx) => tx.tx_hash(signature),
            // Eip8130's hash doesn't depend on an external signature
            // — the full 2718 body (including embedded sender_auth)
            // is the pre-image, handled by `Sealable::hash_slow`.
            Self::Eip8130(tx) => tx.hash_slow(),
            Self::Deposit(tx) => tx.tx_hash(),
        }
    }

    /// Convenience function to convert this typed transaction into an [`OpTxEnvelope`].
    ///
    /// Note: If this is a [`OpTypedTransaction::Deposit`] variant, the signature will be ignored.
    pub fn into_envelope(self, signature: Signature) -> OpTxEnvelope {
        self.into_signed(signature).into()
    }

    /// Attempts to convert the optimism variant into an ethereum [`TypedTransaction`].
    ///
    /// Returns the typed transaction as error if it is a variant unsupported on ethereum:
    /// [`TxDeposit`]
    pub fn try_into_eth(self) -> Result<TypedTransaction, ValueError<Self>> {
        self.try_into_eth_variant()
    }

    /// Attempts to convert the optimism variant into an ethereum [`TypedTransaction`].
    ///
    /// Returns the typed transaction as error if it is a variant unsupported on ethereum:
    /// [`TxDeposit`]
    pub fn try_into_eth_variant<Eip4844>(
        self,
    ) -> Result<EthereumTypedTransaction<Eip4844>, ValueError<Self>> {
        match self {
            Self::Legacy(tx) => Ok(tx.into()),
            Self::Eip2930(tx) => Ok(tx.into()),
            Self::Eip1559(tx) => Ok(tx.into()),
            Self::Eip7702(tx) => Ok(tx.into()),
            tx @ (Self::Eip8130(_) | Self::Deposit(_)) => Err(ValueError::new(
                tx,
                "Deposit / Eip8130 transactions cannot be converted to ethereum transaction",
            )),
        }
    }
}

// For all the following RlpEcdsaEncodableTx / SignableTransaction
// methods: `TxEip8130` carries embedded auth rather than an external
// ECDSA signature, so the `signature: &Signature` parameter is moot
// for the Eip8130 arms — we route through the standalone
// `Encodable2718` / `Encodable` impls on `TxEip8130` and ignore the
// signature. Same pattern Deposit uses.
impl RlpEcdsaEncodableTx for OpTypedTransaction {
    fn rlp_encoded_fields_length(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip2930(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip1559(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip7702(tx) => tx.rlp_encoded_fields_length(),
            // TxEip8130's 2718 body IS the RLP list; use total
            // RLP length minus the 1-byte type prefix.
            Self::Eip8130(tx) => tx.length(),
            Self::Deposit(tx) => tx.rlp_encoded_fields_length(),
        }
    }

    fn rlp_encode_fields(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            Self::Legacy(tx) => tx.rlp_encode_fields(out),
            Self::Eip2930(tx) => tx.rlp_encode_fields(out),
            Self::Eip1559(tx) => tx.rlp_encode_fields(out),
            Self::Eip7702(tx) => tx.rlp_encode_fields(out),
            Self::Eip8130(tx) => tx.encode(out),
            Self::Deposit(tx) => tx.rlp_encode_fields(out),
        }
    }

    fn eip2718_encode_with_type(&self, signature: &Signature, _ty: u8, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip2930(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip1559(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip7702(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip8130(tx) => tx.encode_2718(out),
            Self::Deposit(tx) => tx.encode_2718(out),
        }
    }

    fn eip2718_encode(&self, signature: &Signature, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.eip2718_encode(signature, out),
            Self::Eip2930(tx) => tx.eip2718_encode(signature, out),
            Self::Eip1559(tx) => tx.eip2718_encode(signature, out),
            Self::Eip7702(tx) => tx.eip2718_encode(signature, out),
            Self::Eip8130(tx) => tx.encode_2718(out),
            Self::Deposit(tx) => tx.encode_2718(out),
        }
    }

    fn network_encode_with_type(&self, signature: &Signature, _ty: u8, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip2930(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip1559(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip7702(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip8130(tx) => tx.network_encode(out),
            Self::Deposit(tx) => tx.network_encode(out),
        }
    }

    fn network_encode(&self, signature: &Signature, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.network_encode(signature, out),
            Self::Eip2930(tx) => tx.network_encode(signature, out),
            Self::Eip1559(tx) => tx.network_encode(signature, out),
            Self::Eip7702(tx) => tx.network_encode(signature, out),
            Self::Eip8130(tx) => tx.network_encode(out),
            Self::Deposit(tx) => tx.network_encode(out),
        }
    }

    fn tx_hash_with_type(&self, signature: &Signature, _ty: u8) -> TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip2930(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip1559(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip7702(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip8130(tx) => tx.hash_slow(),
            Self::Deposit(tx) => tx.tx_hash(),
        }
    }

    fn tx_hash(&self, signature: &Signature) -> TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash(signature),
            Self::Eip2930(tx) => tx.tx_hash(signature),
            Self::Eip1559(tx) => tx.tx_hash(signature),
            Self::Eip7702(tx) => tx.tx_hash(signature),
            Self::Eip8130(tx) => tx.hash_slow(),
            Self::Deposit(tx) => tx.tx_hash(),
        }
    }
}

impl SignableTransaction<Signature> for OpTypedTransaction {
    fn set_chain_id(&mut self, chain_id: ChainId) {
        match self {
            Self::Legacy(tx) => tx.set_chain_id(chain_id),
            Self::Eip2930(tx) => tx.set_chain_id(chain_id),
            Self::Eip1559(tx) => tx.set_chain_id(chain_id),
            Self::Eip7702(tx) => tx.set_chain_id(chain_id),
            Self::Eip8130(tx) => tx.chain_id = chain_id,
            Self::Deposit(_) => {}
        }
    }

    fn encode_for_signing(&self, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.encode_for_signing(out),
            Self::Eip2930(tx) => tx.encode_for_signing(out),
            Self::Eip1559(tx) => tx.encode_for_signing(out),
            Self::Eip7702(tx) => tx.encode_for_signing(out),
            // Eip8130 isn't externally ECDSA-signed — the body
            // already carries `sender_auth`/`payer_auth`. Callers
            // that need a "signable" byte string should reach for
            // `sender_signature_hash` / `payer_signature_hash`
            // directly, not this trait method.
            Self::Eip8130(_) | Self::Deposit(_) => {}
        }
    }

    fn payload_len_for_signature(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.payload_len_for_signature(),
            Self::Eip2930(tx) => tx.payload_len_for_signature(),
            Self::Eip1559(tx) => tx.payload_len_for_signature(),
            Self::Eip7702(tx) => tx.payload_len_for_signature(),
            Self::Eip8130(_) | Self::Deposit(_) => 0,
        }
    }

    fn into_signed(self, signature: Signature) -> Signed<Self, Signature>
    where
        Self: Sized,
    {
        let hash = self.tx_hash(&signature);
        Signed::new_unchecked(self, signature, hash)
    }
}
