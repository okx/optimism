//! XLayer transaction extensions.

use alloc::vec::Vec;

use alloy_consensus::{Sealable, Transaction, Typed2718};
use alloy_eips::{
    eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718, IsTyped2718},
    eip2930::AccessList,
    eip7702::SignedAuthorization,
};
use alloy_primitives::{Address, B256, Bytes, ChainId, TxHash, TxKind, U256, keccak256};
use alloy_rlp::{BufMut, Decodable, Encodable, Header, length_of_length};

use crate::transaction::tx_type::{AA_PAYER_TYPE_ID, AA_TX_TYPE_ID};

/// A single call inside an EIP-8130 phase.
///
/// Distinct from [`op_revm::transaction::eip8130::Eip8130Call`], which is the EVM-execution form
/// (carries `value`). This consensus-layer entry has only `to` and `data` because EIP-8130 calls
/// do not carry a top-level value at the protocol level.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Eip8130CallEntry {
    /// Call target.
    pub to: Address,
    /// Calldata.
    pub data: Bytes,
}

impl Encodable for Eip8130CallEntry {
    fn encode(&self, out: &mut dyn BufMut) {
        let payload = self.to.length() + self.data.length();
        Header { list: true, payload_length: payload }.encode(out);
        self.to.encode(out);
        self.data.encode(out);
    }

    fn length(&self) -> usize {
        let payload = self.to.length() + self.data.length();
        payload + length_of_length(payload)
    }
}

impl Decodable for Eip8130CallEntry {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let remaining = buf.len();
        let this = Self { to: Decodable::decode(buf)?, data: Decodable::decode(buf)? };
        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }
        Ok(this)
    }
}

/// Initial account owner registered by a create entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Owner {
    /// Verifier contract address.
    pub verifier: Address,
    /// Verifier-derived owner id.
    pub owner_id: B256,
    /// Permission bitmask.
    pub scope: u8,
}

impl Encodable for Owner {
    fn encode(&self, out: &mut dyn BufMut) {
        let payload = self.verifier.length() + self.owner_id.length() + self.scope.length();
        Header { list: true, payload_length: payload }.encode(out);
        self.verifier.encode(out);
        self.owner_id.encode(out);
        self.scope.encode(out);
    }

    fn length(&self) -> usize {
        let payload = self.verifier.length() + self.owner_id.length() + self.scope.length();
        payload + length_of_length(payload)
    }
}

impl Decodable for Owner {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let remaining = buf.len();
        let this = Self {
            verifier: Decodable::decode(buf)?,
            owner_id: Decodable::decode(buf)?,
            scope: Decodable::decode(buf)?,
        };
        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }
        Ok(this)
    }
}

/// Owner configuration change operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct OwnerChange {
    /// `0x01` authorize, `0x02` revoke.
    pub change_type: u8,
    /// Verifier contract address.
    pub verifier: Address,
    /// Verifier-derived owner id.
    pub owner_id: B256,
    /// Permission bitmask.
    pub scope: u8,
}

impl Encodable for OwnerChange {
    fn encode(&self, out: &mut dyn BufMut) {
        let payload = self.change_type.length()
            + self.verifier.length()
            + self.owner_id.length()
            + self.scope.length();
        Header { list: true, payload_length: payload }.encode(out);
        self.change_type.encode(out);
        self.verifier.encode(out);
        self.owner_id.encode(out);
        self.scope.encode(out);
    }

    fn length(&self) -> usize {
        let payload = self.change_type.length()
            + self.verifier.length()
            + self.owner_id.length()
            + self.scope.length();
        payload + length_of_length(payload)
    }
}

impl Decodable for OwnerChange {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let remaining = buf.len();
        let this = Self {
            change_type: Decodable::decode(buf)?,
            verifier: Decodable::decode(buf)?,
            owner_id: Decodable::decode(buf)?,
            scope: Decodable::decode(buf)?,
        };
        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }
        Ok(this)
    }
}

/// Account creation entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct CreateEntry {
    /// User supplied CREATE2 salt.
    pub user_salt: B256,
    /// Account bytecode.
    pub bytecode: Bytes,
    /// Initial owners.
    pub initial_owners: Vec<Owner>,
}

/// Account configuration change entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct ConfigChangeEntry {
    /// Target chain id, `0` for multichain changes.
    pub chain_id: u64,
    /// Expected sequence number.
    pub sequence: u64,
    /// Owner changes.
    pub owner_changes: Vec<OwnerChange>,
    /// Authorizer authentication data.
    pub authorizer_auth: Bytes,
}

/// EIP-7702-style delegation change entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct DelegationEntry {
    /// Delegation target, or zero address to clear.
    pub target: Address,
}

/// Account-change entry discriminator for create entries.
const CHANGE_TYPE_CREATE: u8 = 0x00;
/// Account-change entry discriminator for config changes.
const CHANGE_TYPE_CONFIG: u8 = 0x01;
/// Account-change entry discriminator for delegation changes.
const CHANGE_TYPE_DELEGATION: u8 = 0x02;

/// An account change entry embedded in an EIP-8130 transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub enum AccountChangeEntry {
    /// Account creation entry.
    Create(CreateEntry),
    /// Account configuration change entry.
    ConfigChange(ConfigChangeEntry),
    /// Delegation change entry.
    Delegation(DelegationEntry),
}

impl Encodable for AccountChangeEntry {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::Create(create) => {
                let owners_len = list_len(&create.initial_owners);
                let payload = CHANGE_TYPE_CREATE.length()
                    + create.user_salt.length()
                    + create.bytecode.length()
                    + owners_len;
                Header { list: true, payload_length: payload }.encode(out);
                CHANGE_TYPE_CREATE.encode(out);
                create.user_salt.encode(out);
                create.bytecode.encode(out);
                encode_list(&create.initial_owners, out);
            }
            Self::ConfigChange(change) => {
                let owner_changes_len = list_len(&change.owner_changes);
                let payload = CHANGE_TYPE_CONFIG.length()
                    + change.chain_id.length()
                    + change.sequence.length()
                    + owner_changes_len
                    + change.authorizer_auth.length();
                Header { list: true, payload_length: payload }.encode(out);
                CHANGE_TYPE_CONFIG.encode(out);
                change.chain_id.encode(out);
                change.sequence.encode(out);
                encode_list(&change.owner_changes, out);
                change.authorizer_auth.encode(out);
            }
            Self::Delegation(delegation) => {
                let payload = CHANGE_TYPE_DELEGATION.length() + delegation.target.length();
                Header { list: true, payload_length: payload }.encode(out);
                CHANGE_TYPE_DELEGATION.encode(out);
                delegation.target.encode(out);
            }
        }
    }

    fn length(&self) -> usize {
        let payload = match self {
            Self::Create(create) => {
                CHANGE_TYPE_CREATE.length()
                    + create.user_salt.length()
                    + create.bytecode.length()
                    + list_len(&create.initial_owners)
            }
            Self::ConfigChange(change) => {
                CHANGE_TYPE_CONFIG.length()
                    + change.chain_id.length()
                    + change.sequence.length()
                    + list_len(&change.owner_changes)
                    + change.authorizer_auth.length()
            }
            Self::Delegation(delegation) => {
                CHANGE_TYPE_DELEGATION.length() + delegation.target.length()
            }
        };
        payload + length_of_length(payload)
    }
}

impl Decodable for AccountChangeEntry {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let remaining = buf.len();
        let ty = u8::decode(buf)?;
        let entry = match ty {
            CHANGE_TYPE_CREATE => Self::Create(CreateEntry {
                user_salt: Decodable::decode(buf)?,
                bytecode: Decodable::decode(buf)?,
                initial_owners: decode_list(buf)?,
            }),
            CHANGE_TYPE_CONFIG => Self::ConfigChange(ConfigChangeEntry {
                chain_id: Decodable::decode(buf)?,
                sequence: Decodable::decode(buf)?,
                owner_changes: decode_list(buf)?,
                authorizer_auth: Decodable::decode(buf)?,
            }),
            CHANGE_TYPE_DELEGATION => {
                Self::Delegation(DelegationEntry { target: Decodable::decode(buf)? })
            }
            _ => return Err(alloy_rlp::Error::Custom("unknown EIP-8130 account change type")),
        };
        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }
        Ok(entry)
    }
}

/// An EIP-8130 account-abstracted transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct TxEip8130 {
    /// Chain id this transaction targets.
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity"))]
    pub chain_id: u64,
    /// Explicit sender. `None` is EOA recovery mode.
    pub from: Option<Address>,
    /// 2D nonce key.
    pub nonce_key: U256,
    /// Sequence number for the nonce key.
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity"))]
    pub nonce_sequence: u64,
    /// Expiry timestamp, or zero for no expiry.
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity"))]
    pub expiry: u64,
    /// EIP-1559 priority fee.
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity"))]
    pub max_priority_fee_per_gas: u128,
    /// EIP-1559 max fee.
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity"))]
    pub max_fee_per_gas: u128,
    /// Execution gas budget.
    #[cfg_attr(
        feature = "serde",
        serde(with = "alloy_serde::quantity", rename = "gas", alias = "gasLimit")
    )]
    pub gas_limit: u64,
    /// Account creation and configuration changes.
    pub account_changes: Vec<AccountChangeEntry>,
    /// Phased call batches.
    pub calls: Vec<Vec<Eip8130CallEntry>>,
    /// Optional payer. `None` means sender pays.
    pub payer: Option<Address>,
    /// Sender authentication data.
    pub sender_auth: Bytes,
    /// Payer authentication data.
    pub payer_auth: Bytes,
}

impl TxEip8130 {
    /// Returns `true` if the sender is recovered from auth data.
    pub const fn is_eoa(&self) -> bool {
        self.from.is_none()
    }

    /// Returns `true` if the sender pays fees.
    pub const fn is_self_pay(&self) -> bool {
        self.payer.is_none()
    }

    /// Effective sender, or zero address for EOA mode before recovery.
    ///
    /// This is only a pre-recovery placeholder and must not be used as the recovered signer.
    pub fn effective_sender(&self) -> Address {
        self.from.unwrap_or(Address::ZERO)
    }

    /// Effective payer.
    pub fn effective_payer(&self) -> Address {
        self.payer.unwrap_or_else(|| self.effective_sender())
    }

    /// Sets the chain id.
    pub fn set_chain_id(&mut self, chain_id: u64) {
        self.chain_id = chain_id;
    }

    /// Transaction hash.
    pub fn tx_hash(&self) -> TxHash {
        let mut buf = Vec::with_capacity(self.encode_2718_len());
        self.encode_2718(&mut buf);
        keccak256(&buf)
    }

    /// Length of RLP-encoded fields without list header.
    pub fn rlp_encoded_fields_length(&self) -> usize {
        self.chain_id.length()
            + optional_address_len(&self.from)
            + self.nonce_key.length()
            + self.nonce_sequence.length()
            + self.expiry.length()
            + self.max_priority_fee_per_gas.length()
            + self.max_fee_per_gas.length()
            + self.gas_limit.length()
            + list_len(&self.account_changes)
            + nested_calls_len(&self.calls)
            + optional_address_len(&self.payer)
            + self.sender_auth.length()
            + self.payer_auth.length()
    }

    /// RLP-encode fields without list header.
    pub fn rlp_encode_fields(&self, out: &mut dyn BufMut) {
        self.chain_id.encode(out);
        encode_optional_address(&self.from, out);
        self.nonce_key.encode(out);
        self.nonce_sequence.encode(out);
        self.expiry.encode(out);
        self.max_priority_fee_per_gas.encode(out);
        self.max_fee_per_gas.encode(out);
        self.gas_limit.encode(out);
        encode_list(&self.account_changes, out);
        encode_nested_calls(&self.calls, out);
        encode_optional_address(&self.payer, out);
        self.sender_auth.encode(out);
        self.payer_auth.encode(out);
    }

    fn rlp_header(&self) -> Header {
        Header { list: true, payload_length: self.rlp_encoded_fields_length() }
    }

    /// RLP-encode this transaction without type prefix.
    pub fn rlp_encode(&self, out: &mut dyn BufMut) {
        self.rlp_header().encode(out);
        self.rlp_encode_fields(out);
    }

    /// RLP-encoded transaction length.
    pub fn rlp_encoded_length(&self) -> usize {
        self.rlp_header().length_with_payload()
    }

    /// EIP-2718 encoded transaction length.
    pub fn eip2718_encoded_length(&self) -> usize {
        self.rlp_encoded_length() + 1
    }

    fn network_header(&self) -> Header {
        Header { list: false, payload_length: self.eip2718_encoded_length() }
    }

    /// Network encoded transaction length.
    pub fn network_encoded_length(&self) -> usize {
        self.network_header().length_with_payload()
    }

    /// Network encode this transaction.
    pub fn network_encode(&self, out: &mut dyn BufMut) {
        self.network_header().encode(out);
        self.encode_2718(out);
    }

    /// RLP-decode this transaction without type prefix.
    pub fn rlp_decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let remaining = buf.len();
        let this = Self {
            chain_id: Decodable::decode(buf)?,
            from: decode_optional_address(buf)?,
            nonce_key: Decodable::decode(buf)?,
            nonce_sequence: Decodable::decode(buf)?,
            expiry: Decodable::decode(buf)?,
            max_priority_fee_per_gas: Decodable::decode(buf)?,
            max_fee_per_gas: Decodable::decode(buf)?,
            gas_limit: Decodable::decode(buf)?,
            account_changes: decode_list(buf)?,
            calls: decode_nested_calls(buf)?,
            payer: decode_optional_address(buf)?,
            sender_auth: Decodable::decode(buf)?,
            payer_auth: Decodable::decode(buf)?,
        };
        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }
        Ok(this)
    }

    /// EIP-8130 signing preimage for `sender_auth`.
    ///
    /// Encodes `AA_TX_TYPE || rlp([chain_id, from, nonce_key, nonce_sequence, expiry,
    /// max_priority_fee_per_gas, max_fee_per_gas, gas_limit, account_changes, calls, payer])`.
    pub fn sender_payload_len_for_signature(&self) -> usize {
        1 + Header {
            list: true,
            payload_length: self.chain_id.length()
                + optional_address_len(&self.from)
                + self.nonce_key.length()
                + self.nonce_sequence.length()
                + self.expiry.length()
                + self.max_priority_fee_per_gas.length()
                + self.max_fee_per_gas.length()
                + self.gas_limit.length()
                + list_len(&self.account_changes)
                + nested_calls_len(&self.calls)
                + optional_address_len(&self.payer),
        }
        .length()
            + self.chain_id.length()
            + optional_address_len(&self.from)
            + self.nonce_key.length()
            + self.nonce_sequence.length()
            + self.expiry.length()
            + self.max_priority_fee_per_gas.length()
            + self.max_fee_per_gas.length()
            + self.gas_limit.length()
            + list_len(&self.account_changes)
            + nested_calls_len(&self.calls)
            + optional_address_len(&self.payer)
    }

    /// Encodes the EIP-8130 sender signing preimage into `out`.
    pub fn encode_for_sender_signing(&self, out: &mut dyn BufMut) {
        let payload_length = self.chain_id.length()
            + optional_address_len(&self.from)
            + self.nonce_key.length()
            + self.nonce_sequence.length()
            + self.expiry.length()
            + self.max_priority_fee_per_gas.length()
            + self.max_fee_per_gas.length()
            + self.gas_limit.length()
            + list_len(&self.account_changes)
            + nested_calls_len(&self.calls)
            + optional_address_len(&self.payer);

        out.put_u8(AA_TX_TYPE_ID);
        Header { list: true, payload_length }.encode(out);
        self.chain_id.encode(out);
        encode_optional_address(&self.from, out);
        self.nonce_key.encode(out);
        self.nonce_sequence.encode(out);
        self.expiry.encode(out);
        self.max_priority_fee_per_gas.encode(out);
        self.max_fee_per_gas.encode(out);
        self.gas_limit.encode(out);
        encode_list(&self.account_changes, out);
        encode_nested_calls(&self.calls, out);
        encode_optional_address(&self.payer, out);
    }

    /// Returns the EIP-8130 sender signing preimage bytes.
    pub fn encoded_for_sender_signing(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.sender_payload_len_for_signature());
        self.encode_for_sender_signing(&mut buf);
        buf
    }

    /// EIP-8130 signing preimage for `payer_auth`.
    ///
    /// Encodes `AA_PAYER_TYPE || rlp([chain_id, from, nonce_key, nonce_sequence, expiry,
    /// max_priority_fee_per_gas, max_fee_per_gas, gas_limit, account_changes, calls])`.
    pub fn payer_payload_len_for_signature(&self) -> usize {
        1 + Header {
            list: true,
            payload_length: self.chain_id.length()
                + optional_address_len(&self.from)
                + self.nonce_key.length()
                + self.nonce_sequence.length()
                + self.expiry.length()
                + self.max_priority_fee_per_gas.length()
                + self.max_fee_per_gas.length()
                + self.gas_limit.length()
                + list_len(&self.account_changes)
                + nested_calls_len(&self.calls),
        }
        .length()
            + self.chain_id.length()
            + optional_address_len(&self.from)
            + self.nonce_key.length()
            + self.nonce_sequence.length()
            + self.expiry.length()
            + self.max_priority_fee_per_gas.length()
            + self.max_fee_per_gas.length()
            + self.gas_limit.length()
            + list_len(&self.account_changes)
            + nested_calls_len(&self.calls)
    }

    /// Encodes the EIP-8130 payer signing preimage into `out`.
    pub fn encode_for_payer_signing(&self, out: &mut dyn BufMut) {
        let payload_length = self.chain_id.length()
            + optional_address_len(&self.from)
            + self.nonce_key.length()
            + self.nonce_sequence.length()
            + self.expiry.length()
            + self.max_priority_fee_per_gas.length()
            + self.max_fee_per_gas.length()
            + self.gas_limit.length()
            + list_len(&self.account_changes)
            + nested_calls_len(&self.calls);

        out.put_u8(AA_PAYER_TYPE_ID);
        Header { list: true, payload_length }.encode(out);
        self.chain_id.encode(out);
        encode_optional_address(&self.from, out);
        self.nonce_key.encode(out);
        self.nonce_sequence.encode(out);
        self.expiry.encode(out);
        self.max_priority_fee_per_gas.encode(out);
        self.max_fee_per_gas.encode(out);
        self.gas_limit.encode(out);
        encode_list(&self.account_changes, out);
        encode_nested_calls(&self.calls, out);
    }

    /// Returns the EIP-8130 payer signing preimage bytes.
    pub fn encoded_for_payer_signing(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.payer_payload_len_for_signature());
        self.encode_for_payer_signing(&mut buf);
        buf
    }

    /// Approximate in-memory size.
    pub fn size(&self) -> usize {
        core::mem::size_of::<Self>()
            + self.account_changes.len() * core::mem::size_of::<AccountChangeEntry>()
            + self
                .calls
                .iter()
                .flat_map(|phase| phase.iter())
                .map(|call| call.data.len())
                .sum::<usize>()
            + self.sender_auth.len()
            + self.payer_auth.len()
    }
}

impl Encodable for TxEip8130 {
    fn encode(&self, out: &mut dyn BufMut) {
        self.rlp_encode(out);
    }

    fn length(&self) -> usize {
        self.rlp_encoded_length()
    }
}

impl Decodable for TxEip8130 {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::rlp_decode(buf)
    }
}

impl Sealable for TxEip8130 {
    fn hash_slow(&self) -> B256 {
        self.tx_hash()
    }
}

impl Typed2718 for TxEip8130 {
    fn ty(&self) -> u8 {
        AA_TX_TYPE_ID
    }
}

impl IsTyped2718 for TxEip8130 {
    fn is_type(ty: u8) -> bool {
        ty == AA_TX_TYPE_ID
    }
}

impl Encodable2718 for TxEip8130 {
    fn type_flag(&self) -> Option<u8> {
        Some(AA_TX_TYPE_ID)
    }

    fn encode_2718_len(&self) -> usize {
        self.eip2718_encoded_length()
    }

    fn encode_2718(&self, out: &mut dyn BufMut) {
        out.put_u8(AA_TX_TYPE_ID);
        self.rlp_encode(out);
    }
}

impl Decodable2718 for TxEip8130 {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        if ty != AA_TX_TYPE_ID {
            return Err(Eip2718Error::UnexpectedType(ty));
        }
        Self::rlp_decode(buf).map_err(Into::into)
    }

    fn fallback_decode(_buf: &mut &[u8]) -> Eip2718Result<Self> {
        Err(Eip2718Error::UnexpectedType(0))
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compact for TxEip8130 {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut rlp = Vec::with_capacity(self.rlp_encoded_length());
        self.rlp_encode(&mut rlp);
        let len = rlp.len();
        buf.put_slice(&rlp);
        len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (input, buf) = <Bytes as reth_codecs::Compact>::from_compact(buf, len);
        let mut slice = input.as_ref();
        let tx = Self::rlp_decode(&mut slice).expect("valid compact EIP-8130 tx");
        (tx, buf)
    }
}

impl Transaction for TxEip8130 {
    fn chain_id(&self) -> Option<ChainId> {
        Some(self.chain_id)
    }

    fn nonce(&self) -> u64 {
        self.nonce_sequence
    }

    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn gas_price(&self) -> Option<u128> {
        None
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.max_fee_per_gas
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        Some(self.max_priority_fee_per_gas)
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        None
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.max_priority_fee_per_gas
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        base_fee.map_or(self.max_fee_per_gas, |base_fee| {
            let base_fee = base_fee as u128;
            base_fee
                + self.max_fee_per_gas.saturating_sub(base_fee).min(self.max_priority_fee_per_gas)
        })
    }

    fn is_dynamic_fee(&self) -> bool {
        true
    }

    fn is_create(&self) -> bool {
        false
    }

    fn kind(&self) -> TxKind {
        // EIP-8130 has no protocol-level `to`; per-call destinations live in `calls[*][*].to`.
        // Returning `Call(ZERO)` signals "no top-level target" to indexers and gas estimators
        // without aliasing the sender (which would be actively misleading).
        TxKind::Call(Address::ZERO)
    }

    fn value(&self) -> U256 {
        U256::ZERO
    }

    fn input(&self) -> &Bytes {
        static EMPTY: Bytes = Bytes::new();
        &EMPTY
    }

    fn access_list(&self) -> Option<&AccessList> {
        None
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        None
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        None
    }

    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        let base_fee = base_fee as u128;
        if self.max_fee_per_gas < base_fee {
            return None;
        }
        Some((self.max_fee_per_gas - base_fee).min(self.max_priority_fee_per_gas))
    }
}

fn encode_optional_address(addr: &Option<Address>, out: &mut dyn BufMut) {
    match addr {
        Some(addr) => addr.encode(out),
        None => out.put_u8(alloy_rlp::EMPTY_STRING_CODE),
    }
}

fn optional_address_len(addr: &Option<Address>) -> usize {
    addr.as_ref().map_or(1, Encodable::length)
}

fn decode_optional_address(buf: &mut &[u8]) -> alloy_rlp::Result<Option<Address>> {
    let header = Header::decode(buf)?;
    if header.list {
        return Err(alloy_rlp::Error::UnexpectedList);
    }
    match header.payload_length {
        0 => Ok(None),
        20 => {
            if buf.len() < 20 {
                return Err(alloy_rlp::Error::InputTooShort);
            }
            let address = Address::from_slice(&buf[..20]);
            *buf = &buf[20..];
            Ok(Some(address))
        }
        _ => Err(alloy_rlp::Error::UnexpectedLength),
    }
}

fn encode_list<T: Encodable>(items: &[T], out: &mut dyn BufMut) {
    let payload_length: usize = items.iter().map(Encodable::length).sum();
    Header { list: true, payload_length }.encode(out);
    for item in items {
        item.encode(out);
    }
}

fn list_len<T: Encodable>(items: &[T]) -> usize {
    let payload_length: usize = items.iter().map(Encodable::length).sum();
    payload_length + length_of_length(payload_length)
}

fn decode_list<T: Decodable>(buf: &mut &[u8]) -> alloy_rlp::Result<Vec<T>> {
    let header = Header::decode(buf)?;
    if !header.list {
        return Err(alloy_rlp::Error::UnexpectedString);
    }
    if header.payload_length > buf.len() {
        return Err(alloy_rlp::Error::InputTooShort);
    }
    let end = buf.len() - header.payload_length;
    let mut items = Vec::new();
    while buf.len() > end {
        items.push(T::decode(buf)?);
    }
    Ok(items)
}

fn encode_nested_calls(phases: &[Vec<Eip8130CallEntry>], out: &mut dyn BufMut) {
    let payload_length: usize = phases.iter().map(|phase| list_len(phase)).sum();
    Header { list: true, payload_length }.encode(out);
    for phase in phases {
        encode_list(phase, out);
    }
}

fn nested_calls_len(phases: &[Vec<Eip8130CallEntry>]) -> usize {
    let payload_length: usize = phases.iter().map(|phase| list_len(phase)).sum();
    payload_length + length_of_length(payload_length)
}

fn decode_nested_calls(buf: &mut &[u8]) -> alloy_rlp::Result<Vec<Vec<Eip8130CallEntry>>> {
    let header = Header::decode(buf)?;
    if !header.list {
        return Err(alloy_rlp::Error::UnexpectedString);
    }
    if header.payload_length > buf.len() {
        return Err(alloy_rlp::Error::InputTooShort);
    }
    let end = buf.len() - header.payload_length;
    let mut phases = Vec::new();
    while buf.len() > end {
        phases.push(decode_list(buf)?);
    }
    Ok(phases)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEPOSIT_TX_TYPE_ID;
    use crate::transaction::xlayer_sig::{payer_signature_hash, sender_signature_hash};
    use alloy_rlp::{Decodable, Encodable};

    fn sample_owner(scope: u8) -> Owner {
        Owner { verifier: Address::repeat_byte(0x41), owner_id: B256::repeat_byte(0x42), scope }
    }

    fn sample_tx() -> TxEip8130 {
        TxEip8130 {
            chain_id: 8453,
            from: Some(Address::repeat_byte(0x01)),
            nonce_key: U256::ZERO,
            nonce_sequence: 42,
            expiry: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 10_000_000_000,
            gas_limit: 100_000,
            account_changes: vec![],
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xBB),
                data: Bytes::from_static(&[0xDE, 0xAD]),
            }]],
            payer: None,
            sender_auth: Bytes::from_static(&[0xFF; 65]),
            payer_auth: Bytes::new(),
        }
    }

    #[test]
    fn rlp_round_trip() {
        let tx = sample_tx();
        let mut buf = Vec::new();
        tx.encode(&mut buf);

        assert_eq!(buf.len(), tx.length());
        assert_eq!(buf.len(), tx.rlp_encoded_length());

        let decoded = TxEip8130::decode(&mut buf.as_slice()).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn empty_tx_rlp_round_trip() {
        let tx = TxEip8130::default();
        let mut buf = Vec::new();
        tx.rlp_encode(&mut buf);

        assert_eq!(buf.len(), tx.rlp_encoded_length());
        let decoded = TxEip8130::rlp_decode(&mut buf.as_slice()).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn eip2718_round_trip() {
        let tx = sample_tx();
        let mut buf = Vec::new();
        tx.encode_2718(&mut buf);

        assert_eq!(buf[0], AA_TX_TYPE_ID);
        assert_eq!(buf.len(), tx.encode_2718_len());
        assert_eq!(tx.type_flag(), Some(AA_TX_TYPE_ID));
        assert!(<TxEip8130 as IsTyped2718>::is_type(AA_TX_TYPE_ID));
        assert!(!<TxEip8130 as IsTyped2718>::is_type(DEPOSIT_TX_TYPE_ID));

        let decoded = TxEip8130::decode_2718(&mut buf.as_slice()).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn eip2718_rejects_unexpected_type() {
        let tx = sample_tx();
        let mut buf = Vec::new();
        tx.rlp_encode(&mut buf);

        let err = TxEip8130::typed_decode(DEPOSIT_TX_TYPE_ID, &mut buf.as_slice()).unwrap_err();
        assert!(matches!(err, Eip2718Error::UnexpectedType(DEPOSIT_TX_TYPE_ID)));
    }

    #[test]
    fn network_encode_wraps_eip2718_payload() {
        let tx = sample_tx();
        let mut network = Vec::new();
        tx.network_encode(&mut network);

        assert_eq!(network.len(), tx.network_encoded_length());

        let header = Header::decode(&mut network.as_slice()).unwrap();
        assert!(!header.list);
        assert_eq!(header.payload_length, tx.eip2718_encoded_length());
    }

    #[test]
    fn transaction_hash_matches_eip2718_keccak() {
        let tx = sample_tx();
        let mut encoded = Vec::new();
        tx.encode_2718(&mut encoded);

        assert_eq!(tx.tx_hash(), keccak256(&encoded));
        assert_eq!(tx.hash_slow(), tx.tx_hash());
    }

    #[test]
    fn tx_trait_getters() {
        let tx = sample_tx();

        assert_eq!(Transaction::chain_id(&tx), Some(8453));
        assert_eq!(tx.nonce(), 42);
        assert_eq!(tx.gas_limit(), 100_000);
        assert!(tx.gas_price().is_none());
        assert_eq!(tx.max_fee_per_gas(), 10_000_000_000);
        assert_eq!(tx.max_priority_fee_per_gas(), Some(1_000_000_000));
        assert_eq!(tx.max_fee_per_blob_gas(), None);
        assert_eq!(tx.priority_fee_or_price(), 1_000_000_000);
        assert_eq!(tx.effective_gas_price(Some(9_500_000_000)), 10_000_000_000);
        assert_eq!(tx.effective_tip_per_gas(9_500_000_000), Some(500_000_000));
        assert_eq!(tx.effective_tip_per_gas(10_500_000_000), None);
        assert!(tx.is_dynamic_fee());
        assert!(!tx.is_create());
        assert_eq!(tx.kind(), TxKind::Call(Address::ZERO));
        assert_eq!(tx.value(), U256::ZERO);
        assert!(tx.input().is_empty());
        assert!(tx.access_list().is_none());
        assert!(tx.blob_versioned_hashes().is_none());
        assert!(tx.authorization_list().is_none());
        assert_eq!(tx.ty(), AA_TX_TYPE_ID);
    }

    #[test]
    fn eoa_and_self_pay_helpers() {
        let mut tx = sample_tx();
        assert!(!tx.is_eoa());
        assert!(tx.is_self_pay());
        assert_eq!(tx.effective_sender(), Address::repeat_byte(0x01));
        assert_eq!(tx.effective_payer(), Address::repeat_byte(0x01));

        tx.from = None;
        assert!(tx.is_eoa());
        assert_eq!(tx.effective_sender(), Address::ZERO);
        assert_eq!(tx.effective_payer(), Address::ZERO);

        tx.payer = Some(Address::repeat_byte(0xCC));
        assert!(!tx.is_self_pay());
        assert_eq!(tx.effective_payer(), Address::repeat_byte(0xCC));
    }

    #[test]
    fn account_creation_entry_round_trip() {
        let tx = TxEip8130 {
            account_changes: vec![AccountChangeEntry::Create(CreateEntry {
                user_salt: B256::repeat_byte(0x01),
                bytecode: Bytes::from_static(&[0x60, 0x80, 0x60, 0x40]),
                initial_owners: vec![sample_owner(0xFF), sample_owner(0x03)],
            })],
            ..sample_tx()
        };

        let mut buf = Vec::new();
        tx.rlp_encode(&mut buf);
        let decoded = TxEip8130::rlp_decode(&mut buf.as_slice()).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn config_change_entry_round_trip() {
        let tx = TxEip8130 {
            account_changes: vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
                chain_id: 8453,
                sequence: 3,
                owner_changes: vec![
                    OwnerChange {
                        change_type: 0x01,
                        verifier: Address::repeat_byte(0x01),
                        owner_id: B256::repeat_byte(0x99),
                        scope: 0x01,
                    },
                    OwnerChange {
                        change_type: 0x02,
                        verifier: Address::ZERO,
                        owner_id: B256::repeat_byte(0x88),
                        scope: 0,
                    },
                ],
                authorizer_auth: Bytes::from(vec![0xEF; 65]),
            })],
            ..sample_tx()
        };

        let mut buf = Vec::new();
        tx.rlp_encode(&mut buf);
        let decoded = TxEip8130::rlp_decode(&mut buf.as_slice()).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn delegation_entry_round_trip() {
        let tx = TxEip8130 {
            account_changes: vec![AccountChangeEntry::Delegation(DelegationEntry {
                target: Address::repeat_byte(0xAA),
            })],
            ..sample_tx()
        };

        let mut buf = Vec::new();
        tx.rlp_encode(&mut buf);
        let decoded = TxEip8130::rlp_decode(&mut buf.as_slice()).unwrap();
        assert_eq!(tx, decoded);
    }

    #[test]
    fn multi_phase_calls_round_trip() {
        let tx = TxEip8130 {
            calls: vec![
                vec![
                    Eip8130CallEntry { to: Address::repeat_byte(1), data: Bytes::from_static(&[0x01]) },
                    Eip8130CallEntry { to: Address::repeat_byte(2), data: Bytes::from_static(&[0x02]) },
                ],
                vec![Eip8130CallEntry { to: Address::repeat_byte(3), data: Bytes::from_static(&[0x03]) }],
            ],
            ..sample_tx()
        };

        let mut buf = Vec::new();
        tx.encode(&mut buf);
        let decoded = TxEip8130::decode(&mut buf.as_slice()).unwrap();
        assert_eq!(decoded, tx);
        assert_eq!(decoded.calls.len(), 2);
        assert_eq!(decoded.calls[0].len(), 2);
        assert_eq!(decoded.calls[1].len(), 1);
    }

    #[test]
    fn optional_address_helpers_round_trip() {
        let mut empty_buf = Vec::new();
        encode_optional_address(&None, &mut empty_buf);
        assert_eq!(empty_buf, vec![alloy_rlp::EMPTY_STRING_CODE]);
        assert_eq!(optional_address_len(&None), empty_buf.len());
        let empty_decoded = decode_optional_address(&mut empty_buf.as_slice()).unwrap();
        assert_eq!(empty_decoded, None);

        let address = Address::repeat_byte(0xAB);
        let mut address_buf = Vec::new();
        encode_optional_address(&Some(address), &mut address_buf);
        assert_eq!(optional_address_len(&Some(address)), address_buf.len());
        let address_decoded = decode_optional_address(&mut address_buf.as_slice()).unwrap();
        assert_eq!(address_decoded, Some(address));
    }

    #[test]
    fn malformed_optional_address_is_rejected() {
        let malformed = [0x83, 0x01, 0x02, 0x03];
        assert!(decode_optional_address(&mut malformed.as_slice()).is_err());
    }

    #[test]
    fn aa_tx_type_is_distinct() {
        assert_eq!(AA_TX_TYPE_ID, 0x7B);
        assert_eq!(AA_PAYER_TYPE_ID, 0x7C);
        assert_ne!(AA_TX_TYPE_ID, AA_PAYER_TYPE_ID);
        assert_ne!(AA_TX_TYPE_ID, 0x00);
        assert_ne!(AA_TX_TYPE_ID, 0x01);
        assert_ne!(AA_TX_TYPE_ID, 0x02);
        assert_ne!(AA_TX_TYPE_ID, 0x04);
        assert_ne!(AA_TX_TYPE_ID, DEPOSIT_TX_TYPE_ID);
    }

    #[test]
    fn signature_hashes_use_eip8130_field_sets() {
        let tx = TxEip8130 {
            nonce_key: U256::from(7),
            expiry: 1_900_000_000,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 10,
            payer: Some(Address::repeat_byte(0x33)),
            sender_auth: Bytes::from(vec![0x44; 65]),
            payer_auth: Bytes::from(vec![0x55; 65]),
            ..sample_tx()
        };

        let mut sender_preimage = Vec::new();
        tx.encode_for_sender_signing(&mut sender_preimage);
        assert_eq!(sender_preimage[0], AA_TX_TYPE_ID);

        let mut payer_preimage = Vec::new();
        tx.encode_for_payer_signing(&mut payer_preimage);
        assert_eq!(payer_preimage[0], AA_PAYER_TYPE_ID);

        let sender_hash = sender_signature_hash(&tx);
        let payer_hash = payer_signature_hash(&tx);
        assert_ne!(sender_hash, payer_hash);
        assert_eq!(sender_hash, keccak256(&sender_preimage));
        assert_eq!(payer_hash, keccak256(&payer_preimage));

        let mut changed_auth = tx.clone();
        changed_auth.sender_auth = Bytes::from(vec![0x66; 65]);
        changed_auth.payer_auth = Bytes::from(vec![0x77; 65]);
        assert_eq!(sender_hash, sender_signature_hash(&changed_auth));
        assert_eq!(payer_hash, payer_signature_hash(&changed_auth));

        let mut changed_payer = tx.clone();
        changed_payer.payer = Some(Address::repeat_byte(0x88));
        assert_ne!(sender_hash, sender_signature_hash(&changed_payer));
        assert_eq!(payer_hash, payer_signature_hash(&changed_payer));

        let mut changed_call = tx;
        changed_call.calls[0][0].data = Bytes::from_static(&[0x99]);
        assert_ne!(sender_hash, sender_signature_hash(&changed_call));
        assert_ne!(payer_hash, payer_signature_hash(&changed_call));
    }

    #[test]
    fn sender_hash_changes_with_nonce() {
        let tx = sample_tx();
        let changed_nonce = TxEip8130 { nonce_sequence: tx.nonce_sequence + 1, ..tx.clone() };
        assert_ne!(sender_signature_hash(&tx), sender_signature_hash(&changed_nonce));
    }

    #[test]
    fn payer_hash_ignores_payer_but_not_nonce() {
        let tx = TxEip8130 { payer: Some(Address::repeat_byte(0x33)), ..sample_tx() };

        let changed_payer = TxEip8130 { payer: Some(Address::repeat_byte(0x44)), ..tx.clone() };
        assert_eq!(payer_signature_hash(&tx), payer_signature_hash(&changed_payer));

        let changed_nonce = TxEip8130 { nonce_sequence: tx.nonce_sequence + 1, ..tx.clone() };
        assert_ne!(payer_signature_hash(&tx), payer_signature_hash(&changed_nonce));
    }

    #[test]
    fn size_counts_dynamic_auth_and_call_data() {
        let tx = sample_tx();
        let expected_floor = core::mem::size_of::<TxEip8130>()
            + tx.account_changes.len() * core::mem::size_of::<AccountChangeEntry>()
            + tx.sender_auth.len()
            + tx.payer_auth.len()
            + tx.calls
                .iter()
                .flat_map(|phase| phase.iter())
                .map(|call| call.data.len())
                .sum::<usize>();

        assert_eq!(tx.size(), expected_floor);
    }
}
