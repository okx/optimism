//! Contains Optimism specific precompiles.
use crate::{
    OpSpecId,
    transaction::{OpTxTr, eip8130::Eip8130Call},
};
use revm::{
    Database,
    context::{Cfg, LocalContextTr},
    context_interface::{ContextTr, Transaction},
    handler::{EthPrecompiles, PrecompileProvider},
    interpreter::{CallInput, CallInputs, Gas, InstructionResult, InterpreterResult},
    precompile::{
        self, EthPrecompileResult, Precompile, PrecompileHalt, PrecompileId, Precompiles, bn254,
        eth_precompile_fn, modexp, secp256r1,
    },
    primitives::{Address, B256, Bytes, OnceLock, U256, hardfork::SpecId, keccak256},
};
use std::{boxed::Box, cell::RefCell, string::String, vec::Vec};

// ── EIP-8130 system precompile constants ────────────────────────────────────────

/// EIP-8130 AA transaction type byte.
const EIP8130_TX_TYPE: u8 = 0x7B;

/// NonceManager system precompile address.
pub const NONCE_MANAGER_ADDRESS: Address =
    Address::new([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xaa, 0x02]);

/// TxContext system precompile address.
pub const TX_CONTEXT_ADDRESS: Address =
    Address::new([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xaa, 0x03]);

/// Base storage slot for NonceManager nonce mapping.
pub const NONCE_BASE_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);

/// Gas cost for TxContext precompile calls.
pub const TX_CONTEXT_GAS: u64 = 100;

/// Gas cost for NonceManager precompile calls.
pub const NONCE_MANAGER_GAS: u64 = 2_100;

// ── EIP-8130 thread-local transaction context ──────────────────────────────────

thread_local! {
    static EIP8130_TX_CONTEXT: RefCell<Option<Eip8130TxContext>> = const { RefCell::new(None) };
}

/// Lightweight snapshot of EIP-8130 tx fields needed by the TxContext precompile.
/// Stored in a thread-local so DynPrecompile closures (which only receive
/// `EvmInternals`) can access them without transient storage.
#[derive(Clone, Debug)]
pub struct Eip8130TxContext {
    /// Effective sender address.
    pub sender: Address,
    /// Effective payer address.
    pub payer: Address,
    /// Owner ID from sender authentication.
    pub owner_id: B256,
    /// Execution-only gas limit (the sender's `gas_limit` field).
    pub gas_limit: u64,
    /// `(gas_limit + known_intrinsic) * max_fee_per_gas`.
    ///
    /// `known_intrinsic` includes all protocol costs computed so far
    /// **except** `payer_auth_cost` (the payer verifier may still be
    /// running when it calls `getMaxCost()`).
    pub max_cost: U256,
    /// Phased call batches.
    pub call_phases: Vec<Vec<Eip8130Call>>,
}

/// Sets the EIP-8130 transaction context for the current thread.
/// Called by the handler before EVM execution of an AA transaction.
pub fn set_eip8130_tx_context(ctx: Eip8130TxContext) {
    EIP8130_TX_CONTEXT.with(|c| *c.borrow_mut() = Some(ctx));
}

/// Clears the EIP-8130 transaction context for the current thread.
/// Called by the handler before processing any transaction (to avoid stale state).
pub fn clear_eip8130_tx_context() {
    EIP8130_TX_CONTEXT.with(|c| *c.borrow_mut() = None);
}

/// Reads the current EIP-8130 transaction context from the thread-local.
/// Returns `None` for non-AA transactions.
pub fn get_eip8130_tx_context() -> Option<Eip8130TxContext> {
    EIP8130_TX_CONTEXT.with(|c| c.borrow().clone())
}

impl Eip8130TxContext {
    /// Builds the context from parts and tx fields.
    ///
    /// `execution_gas_limit` is the sender's execution-only `gas_limit`.
    /// `known_intrinsic` is the intrinsic gas minus payer auth costs (since
    /// the payer verifier may call `getMaxCost()` while still running).
    pub fn new(
        parts: &crate::transaction::eip8130::Eip8130Parts,
        execution_gas_limit: u64,
        known_intrinsic: u64,
        max_fee_per_gas: U256,
    ) -> Self {
        let total_gas = U256::from(execution_gas_limit)
            + U256::from(known_intrinsic)
            + U256::from(parts.custom_verifier_gas_cap);
        Self {
            sender: parts.sender,
            payer: parts.payer,
            owner_id: parts.owner_id,
            gas_limit: execution_gas_limit,
            max_cost: total_gas * max_fee_per_gas,
            call_phases: parts.call_phases.clone(),
        }
    }
}

/// Returns `true` when the EIP-8130 system precompiles are active for the given spec.
fn eip8130_precompiles_enabled(spec: OpSpecId) -> bool {
    matches!(spec, OpSpecId::NATIVE_AA)
}

/// Computes the 4-byte function selector from a Solidity signature.
pub fn selector(sig: &[u8]) -> [u8; 4] {
    let h = keccak256(sig);
    [h[0], h[1], h[2], h[3]]
}

/// Computes the NonceManager storage slot for `nonce[account][nonce_key]`.
pub fn aa_nonce_slot(account: Address, nonce_key: U256) -> U256 {
    let inner = {
        let mut buf = [0u8; 64];
        buf[12..32].copy_from_slice(account.as_slice());
        let base_bytes = NONCE_BASE_SLOT.to_be_bytes::<32>();
        buf[32..64].copy_from_slice(&base_bytes);
        keccak256(buf)
    };

    let outer = {
        let mut buf = [0u8; 64];
        buf[0..32].copy_from_slice(&nonce_key.to_be_bytes::<32>());
        buf[32..64].copy_from_slice(inner.as_slice());
        keccak256(buf)
    };

    U256::from_be_bytes(outer.0)
}

/// ABI-encodes an address as a left-padded 32-byte word.
pub fn encode_address(address: Address) -> Bytes {
    let mut out = [0u8; 32];
    out[12..32].copy_from_slice(address.as_slice());
    Bytes::from(out.to_vec())
}

/// ABI-encodes a U256 as a big-endian 32-byte word.
pub fn encode_u256(value: U256) -> Bytes {
    Bytes::from(value.to_be_bytes::<32>().to_vec())
}

/// ABI-encodes a raw 32-byte value.
pub fn encode_b256(value: [u8; 32]) -> Bytes {
    Bytes::from(value.to_vec())
}

/// ABI-encodes the return value of `getCalls() returns (CallTuple[][] memory)`
/// where `CallTuple = (address target, bytes data)`.
///
/// Encoding layout (standard Solidity ABI for a dynamic nested array of structs):
/// - word 0: offset to outer array = 0x20
/// - outer array: [length, offset_0, offset_1, ..., phase_0_data, phase_1_data, ...]
/// - each phase: [length, offset_0, ..., struct_0, struct_1, ...]
/// - each struct: [address (padded), offset_to_bytes (0x40), bytes_len, bytes_data (padded)]
pub fn encode_calls_abi(phases: &[Vec<Eip8130Call>]) -> Bytes {
    let mut buf = Vec::<u8>::new();

    // Return value offset (points to the outer array).
    buf.extend_from_slice(&pad32(32u64));

    // Outer array length.
    buf.extend_from_slice(&pad32(phases.len() as u64));

    // Pre-compute offsets to each phase's data (relative to outer array body start).
    // Body starts right after the N offset words.
    let mut phase_offsets = Vec::with_capacity(phases.len());
    let mut running = phases.len() * 32; // skip N offset words
    for phase in phases {
        phase_offsets.push(running);
        running += encoded_phase_size(phase);
    }

    for off in &phase_offsets {
        buf.extend_from_slice(&pad32(*off as u64));
    }

    for phase in phases {
        encode_phase(phase, &mut buf);
    }

    Bytes::from(buf)
}

fn encoded_struct_size(call: &Eip8130Call) -> usize {
    // address (32) + offset to bytes (32) + bytes length (32) + padded data
    32 + 32 + 32 + padded_len(call.data.len())
}

fn encoded_phase_size(phase: &[Eip8130Call]) -> usize {
    let m = phase.len();
    // length word + M offset words + sum of struct sizes
    32 + m * 32 + phase.iter().map(|c| encoded_struct_size(c)).sum::<usize>()
}

fn encode_phase(phase: &[Eip8130Call], buf: &mut Vec<u8>) {
    let m = phase.len();
    buf.extend_from_slice(&pad32(m as u64));

    // Offsets to each struct (relative to the start of the struct data area).
    let mut struct_offsets = Vec::with_capacity(m);
    let mut running = m * 32; // skip M offset words
    for call in phase {
        struct_offsets.push(running);
        running += encoded_struct_size(call);
    }

    for off in &struct_offsets {
        buf.extend_from_slice(&pad32(*off as u64));
    }

    for call in phase {
        // address (left-padded to 32)
        let mut addr_word = [0u8; 32];
        addr_word[12..32].copy_from_slice(call.to.as_slice());
        buf.extend_from_slice(&addr_word);

        // offset to bytes data = 0x40 (2 words past struct head start)
        buf.extend_from_slice(&pad32(64u64));

        // bytes: length + right-padded data
        buf.extend_from_slice(&pad32(call.data.len() as u64));
        buf.extend_from_slice(&call.data);
        let pad = padded_len(call.data.len()) - call.data.len();
        buf.extend(std::iter::repeat(0u8).take(pad));
    }
}

fn pad32(v: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..32].copy_from_slice(&v.to_be_bytes());
    word
}

fn padded_len(len: usize) -> usize {
    (len + 31) & !31
}

fn map_precompile_output(
    gas_limit: u64,
    output: Result<(u64, Bytes), String>,
) -> InterpreterResult {
    let mut result = InterpreterResult {
        result: InstructionResult::Return,
        gas: Gas::new(gas_limit),
        output: Bytes::new(),
    };

    match output {
        Ok((gas_used, bytes)) => {
            if gas_limit < gas_used {
                result.result = InstructionResult::PrecompileOOG;
            } else {
                // revm 38 renamed `record_cost` → `record_regular_cost`.
                let enough_gas = result.gas.record_regular_cost(gas_used);
                debug_assert!(enough_gas, "gas should be sufficient after explicit limit check");
                result.output = bytes;
            }
        }
        Err(_) => {
            result.result = InstructionResult::PrecompileError;
        }
    }

    result
}

fn run_nonce_manager_precompile<CTX>(
    context: &mut CTX,
    input: &[u8],
) -> Result<(u64, Bytes), String>
where
    CTX: ContextTr<Cfg: Cfg<Spec = OpSpecId>, Tx: OpTxTr>,
{
    let get_nonce_selector = selector(b"getNonce(address,uint256)");
    if input.len() < 4 || input[0..4] != get_nonce_selector {
        return Err("unknown nonce manager selector".to_string());
    }
    if input.len() < 4 + 32 + 32 {
        return Err("invalid nonce manager input".to_string());
    }

    let account = Address::from_slice(&input[4 + 12..4 + 32]);
    let nonce_key = U256::from_be_slice(&input[4 + 32..4 + 64]);
    let slot = aa_nonce_slot(account, nonce_key);

    let storage_value =
        context.db_mut().storage(NONCE_MANAGER_ADDRESS, slot.into()).map_err(|e| e.to_string())?;

    let mut out = [0u8; 32];
    let storage_bytes = storage_value.to_be_bytes::<32>();
    out[24..32].copy_from_slice(&storage_bytes[24..32]);

    Ok((NONCE_MANAGER_GAS, Bytes::from(out.to_vec())))
}

fn run_tx_context_precompile<CTX>(context: &CTX, input: &[u8]) -> Result<(u64, Bytes), String>
where
    CTX: ContextTr<Cfg: Cfg<Spec = OpSpecId>, Tx: OpTxTr>,
{
    if input.len() < 4 {
        return Err("invalid tx context input".to_string());
    }

    let tx = context.tx();
    let eip8130 =
        if tx.tx_type() == EIP8130_TX_TYPE { Some(tx.eip8130_parts().clone()) } else { None };

    let selector_bytes = &input[0..4];
    let output = if selector_bytes == selector(b"getSender()") {
        encode_address(eip8130.as_ref().map_or(Address::ZERO, |p| p.sender))
    } else if selector_bytes == selector(b"getPayer()") {
        encode_address(eip8130.as_ref().map_or(Address::ZERO, |p| p.payer))
    } else if selector_bytes == selector(b"getOwnerId()") {
        encode_b256(eip8130.as_ref().map_or([0u8; 32], |p| p.owner_id.0))
    } else if selector_bytes == selector(b"getMaxCost()") {
        let max_cost = get_eip8130_tx_context().map_or(U256::ZERO, |ctx| ctx.max_cost);
        encode_u256(max_cost)
    } else if selector_bytes == selector(b"getGasLimit()") {
        let gas_limit = get_eip8130_tx_context().map_or(0u64, |ctx| ctx.gas_limit);
        encode_u256(U256::from(gas_limit))
    } else if selector_bytes == selector(b"getCalls()") {
        let phases = eip8130.as_ref().map_or(&[][..], |p| &p.call_phases);
        encode_calls_abi(phases)
    } else {
        return Err("unknown tx context selector".to_string());
    };

    Ok((TX_CONTEXT_GAS, output))
}

/// Optimism precompile provider
#[derive(Debug, Clone)]
pub struct OpPrecompiles {
    /// Inner precompile provider is same as Ethereums.
    inner: EthPrecompiles,
    /// Spec id of the precompile provider.
    spec: OpSpecId,
}

impl OpPrecompiles {
    /// Create a new precompile provider with the given `OpSpec`.
    #[inline]
    pub fn new_with_spec(spec: OpSpecId) -> Self {
        let precompiles = match spec {
            spec @ (OpSpecId::BEDROCK |
            OpSpecId::REGOLITH |
            OpSpecId::CANYON |
            OpSpecId::ECOTONE) => Precompiles::new(spec.into_eth_spec().into()),
            OpSpecId::FJORD => fjord(),
            OpSpecId::GRANITE | OpSpecId::HOLOCENE => granite(),
            OpSpecId::ISTHMUS => isthmus(),
            OpSpecId::JOVIAN => jovian(),
            OpSpecId::KARST | OpSpecId::INTEROP => karst(),
            // NATIVE_AA inherits KARST's standard precompile set. The two EIP-8130
            // native precompiles (NonceManager at 0x..aa02, TxContext at 0x..aa03) require
            // EVM-database / AA-transaction-context access that revm's static `Precompile`
            // signature cannot carry, so they're dispatched via call interception at the
            // AA execution layer in handler.rs, not registered into this static set.
            OpSpecId::NATIVE_AA => karst(),
        };

        Self { inner: EthPrecompiles { precompiles, spec: SpecId::default() }, spec }
    }

    /// Precompiles getter.
    #[inline]
    pub const fn precompiles(&self) -> &'static Precompiles {
        self.inner.precompiles
    }
}

/// Returns precompiles for Fjord spec.
pub fn fjord() -> &'static Precompiles {
    static INSTANCE: OnceLock<Precompiles> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = Precompiles::cancun().clone();
        // RIP-7212: secp256r1 P256verify
        precompiles.extend([secp256r1::P256VERIFY]);
        precompiles
    })
}

/// Returns precompiles for Granite spec.
pub fn granite() -> &'static Precompiles {
    static INSTANCE: OnceLock<Precompiles> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = fjord().clone();
        // Restrict bn254Pairing input size
        precompiles.extend([bn254_pair::GRANITE]);
        precompiles
    })
}

/// Returns precompiles for isthmus spec.
pub fn isthmus() -> &'static Precompiles {
    static INSTANCE: OnceLock<Precompiles> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = granite().clone();
        // Prague bls12 precompiles
        precompiles.extend(precompile::bls12_381::precompiles());
        // Isthmus bls12 precompile modifications
        precompiles.extend([
            bls12_381::ISTHMUS_G1_MSM,
            bls12_381::ISTHMUS_G2_MSM,
            bls12_381::ISTHMUS_PAIRING,
        ]);
        precompiles
    })
}

/// Returns precompiles for karst spec.
pub fn karst() -> &'static Precompiles {
    static INSTANCE: OnceLock<Precompiles> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = jovian().clone();

        let mut to_remove = Precompiles::default();
        to_remove.extend([modexp::BERLIN, secp256r1::P256VERIFY]);

        precompiles.difference(&to_remove);

        precompiles.extend([modexp::OSAKA, secp256r1::P256VERIFY_OSAKA]);

        precompiles
    })
}

/// Returns precompiles for jovian spec.
pub fn jovian() -> &'static Precompiles {
    static INSTANCE: OnceLock<Precompiles> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = isthmus().clone();

        let mut to_remove = Precompiles::default();
        to_remove.extend([
            bn254::pair::ISTANBUL,
            bls12_381::ISTHMUS_G1_MSM,
            bls12_381::ISTHMUS_G2_MSM,
            bls12_381::ISTHMUS_PAIRING,
        ]);

        // Replace the 4 variable-input precompiles with Jovian versions (reduced limits)
        precompiles.difference(&to_remove);

        precompiles.extend([
            bn254_pair::JOVIAN,
            bls12_381::JOVIAN_G1_MSM,
            bls12_381::JOVIAN_G2_MSM,
            bls12_381::JOVIAN_PAIRING,
        ]);

        precompiles
    })
}

impl<CTX> PrecompileProvider<CTX> for OpPrecompiles
where
    CTX: ContextTr<Cfg: Cfg<Spec = OpSpecId>, Tx: OpTxTr>,
{
    type Output = InterpreterResult;

    #[inline]
    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        if spec == self.spec {
            return false;
        }
        *self = Self::new_with_spec(spec);
        true
    }

    #[inline]
    fn run(
        &mut self,
        context: &mut CTX,
        inputs: &CallInputs,
    ) -> Result<Option<Self::Output>, String> {
        let aa_context = context.tx().tx_type() == EIP8130_TX_TYPE;
        if eip8130_precompiles_enabled(self.spec) || aa_context {
            let input_bytes: std::vec::Vec<u8> = match &inputs.input {
                CallInput::SharedBuffer(range) => context
                    .local()
                    .shared_memory_buffer_slice(range.clone())
                    .map(|slice| slice.to_vec())
                    .unwrap_or_default(),
                CallInput::Bytes(bytes) => bytes.to_vec(),
            };

            if inputs.bytecode_address == NONCE_MANAGER_ADDRESS {
                let output = run_nonce_manager_precompile(context, &input_bytes);
                return Ok(Some(map_precompile_output(inputs.gas_limit, output)));
            }

            if inputs.bytecode_address == TX_CONTEXT_ADDRESS {
                let output = run_tx_context_precompile(context, &input_bytes);
                return Ok(Some(map_precompile_output(inputs.gas_limit, output)));
            }
        }

        self.inner.run(context, inputs)
    }

    #[inline]
    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        let mut addresses: std::vec::Vec<Address> = self.inner.warm_addresses().collect();
        if eip8130_precompiles_enabled(self.spec) {
            addresses.push(NONCE_MANAGER_ADDRESS);
            addresses.push(TX_CONTEXT_ADDRESS);
        }
        Box::new(addresses.into_iter())
    }

    #[inline]
    fn contains(&self, address: &Address) -> bool {
        (eip8130_precompiles_enabled(self.spec)
            && (*address == NONCE_MANAGER_ADDRESS || *address == TX_CONTEXT_ADDRESS))
            || self.inner.contains(address)
    }
}

impl Default for OpPrecompiles {
    fn default() -> Self {
        Self::new_with_spec(OpSpecId::JOVIAN)
    }
}

/// Bn254 pair precompile.
pub mod bn254_pair {
    use super::*;

    /// Max input size for the bn254 pair precompile.
    pub const GRANITE_MAX_INPUT_SIZE: usize = 112687;
    /// Bn254 pair precompile.
    pub const GRANITE: Precompile =
        Precompile::new(PrecompileId::Bn254Pairing, bn254::pair::ADDRESS, granite_precompile);

    /// Run the bn254 pair precompile with Optimism input limit.
    pub fn run_pair_granite(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
        if input.len() > GRANITE_MAX_INPUT_SIZE {
            return Err(PrecompileHalt::Bn254PairLength);
        }
        bn254::run_pair(
            input,
            bn254::pair::ISTANBUL_PAIR_PER_POINT,
            bn254::pair::ISTANBUL_PAIR_BASE,
            gas_limit,
        )
    }

    eth_precompile_fn!(granite_precompile, run_pair_granite);

    /// Max input size for the bn254 pair precompile.
    pub const JOVIAN_MAX_INPUT_SIZE: usize = 81_984;
    /// Bn254 pair precompile.
    pub const JOVIAN: Precompile =
        Precompile::new(PrecompileId::Bn254Pairing, bn254::pair::ADDRESS, jovian_precompile);

    /// Run the bn254 pair precompile with Optimism input limit.
    pub fn run_pair_jovian(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
        if input.len() > JOVIAN_MAX_INPUT_SIZE {
            return Err(PrecompileHalt::Bn254PairLength);
        }
        bn254::run_pair(
            input,
            bn254::pair::ISTANBUL_PAIR_PER_POINT,
            bn254::pair::ISTANBUL_PAIR_BASE,
            gas_limit,
        )
    }

    eth_precompile_fn!(jovian_precompile, run_pair_jovian);
}

/// `Bls12_381` precompile.
pub mod bls12_381 {
    use super::*;
    use revm::precompile::bls12_381_const::{G1_MSM_ADDRESS, G2_MSM_ADDRESS, PAIRING_ADDRESS};

    /// Max input size for the g1 msm precompile.
    pub const ISTHMUS_G1_MSM_MAX_INPUT_SIZE: usize = 513760;

    /// The maximum input size for the BLS12-381 g1 msm operation after the Jovian Hardfork.
    pub const JOVIAN_G1_MSM_MAX_INPUT_SIZE: usize = 288_960;

    /// Max input size for the g2 msm precompile.
    pub const ISTHMUS_G2_MSM_MAX_INPUT_SIZE: usize = 488448;

    /// Max input size for the g2 msm precompile after the Jovian Hardfork.
    pub const JOVIAN_G2_MSM_MAX_INPUT_SIZE: usize = 278_784;

    /// Max input size for the pairing precompile.
    pub const ISTHMUS_PAIRING_MAX_INPUT_SIZE: usize = 235008;

    /// Max input size for the pairing precompile after the Jovian Hardfork.
    pub const JOVIAN_PAIRING_MAX_INPUT_SIZE: usize = 156_672;

    /// G1 msm precompile.
    pub const ISTHMUS_G1_MSM: Precompile =
        Precompile::new(PrecompileId::Bls12G1Msm, G1_MSM_ADDRESS, isthmus_g1_msm_precompile);
    /// G2 msm precompile.
    pub const ISTHMUS_G2_MSM: Precompile =
        Precompile::new(PrecompileId::Bls12G2Msm, G2_MSM_ADDRESS, isthmus_g2_msm_precompile);
    /// Pairing precompile.
    pub const ISTHMUS_PAIRING: Precompile =
        Precompile::new(PrecompileId::Bls12Pairing, PAIRING_ADDRESS, isthmus_pairing_precompile);

    /// G1 msm precompile after the Jovian Hardfork.
    pub const JOVIAN_G1_MSM: Precompile =
        Precompile::new(PrecompileId::Bls12G1Msm, G1_MSM_ADDRESS, jovian_g1_msm_precompile);
    /// G2 msm precompile after the Jovian Hardfork.
    pub const JOVIAN_G2_MSM: Precompile =
        Precompile::new(PrecompileId::Bls12G2Msm, G2_MSM_ADDRESS, jovian_g2_msm_precompile);
    /// Pairing precompile after the Jovian Hardfork.
    pub const JOVIAN_PAIRING: Precompile =
        Precompile::new(PrecompileId::Bls12Pairing, PAIRING_ADDRESS, jovian_pairing_precompile);

    eth_precompile_fn!(isthmus_g1_msm_precompile, run_g1_msm_isthmus);
    eth_precompile_fn!(isthmus_g2_msm_precompile, run_g2_msm_isthmus);
    eth_precompile_fn!(isthmus_pairing_precompile, run_pair_isthmus);
    eth_precompile_fn!(jovian_g1_msm_precompile, run_g1_msm_jovian);
    eth_precompile_fn!(jovian_g2_msm_precompile, run_g2_msm_jovian);
    eth_precompile_fn!(jovian_pairing_precompile, run_pair_jovian);

    /// Run the g1 msm precompile with Optimism input limit.
    pub fn run_g1_msm_isthmus(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
        if input.len() > ISTHMUS_G1_MSM_MAX_INPUT_SIZE {
            return Err(PrecompileHalt::Other(
                "G1MSM input length too long for OP Stack input size limitation after the Isthmus Hardfork".into(),
            ));
        }
        precompile::bls12_381::g1_msm::g1_msm(input, gas_limit)
    }

    /// Run the g1 msm precompile with Optimism input limit.
    pub fn run_g1_msm_jovian(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
        if input.len() > JOVIAN_G1_MSM_MAX_INPUT_SIZE {
            return Err(PrecompileHalt::Other(
                "G1MSM input length too long for OP Stack input size limitation after the Jovian Hardfork".into(),
            ));
        }
        precompile::bls12_381::g1_msm::g1_msm(input, gas_limit)
    }

    /// Run the g2 msm precompile with Optimism input limit.
    pub fn run_g2_msm_isthmus(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
        if input.len() > ISTHMUS_G2_MSM_MAX_INPUT_SIZE {
            return Err(PrecompileHalt::Other(
                "G2MSM input length too long for OP Stack input size limitation".into(),
            ));
        }
        precompile::bls12_381::g2_msm::g2_msm(input, gas_limit)
    }

    /// Run the g2 msm precompile with Optimism input limit after the Jovian Hardfork.
    pub fn run_g2_msm_jovian(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
        if input.len() > JOVIAN_G2_MSM_MAX_INPUT_SIZE {
            return Err(PrecompileHalt::Other(
                "G2MSM input length too long for OP Stack input size limitation after the Jovian Hardfork".into(),
            ));
        }
        precompile::bls12_381::g2_msm::g2_msm(input, gas_limit)
    }

    /// Run the pairing precompile with Optimism input limit.
    pub fn run_pair_isthmus(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
        if input.len() > ISTHMUS_PAIRING_MAX_INPUT_SIZE {
            return Err(PrecompileHalt::Other(
                "Pairing input length too long for OP Stack input size limitation".into(),
            ));
        }
        precompile::bls12_381::pairing::pairing(input, gas_limit)
    }

    /// Run the pairing precompile with Optimism input limit after the Jovian Hardfork.
    pub fn run_pair_jovian(input: &[u8], gas_limit: u64) -> EthPrecompileResult {
        if input.len() > JOVIAN_PAIRING_MAX_INPUT_SIZE {
            return Err(PrecompileHalt::Other(
                "Pairing input length too long for OP Stack input size limitation after the Jovian Hardfork".into(),
            ));
        }
        precompile::bls12_381::pairing::pairing(input, gas_limit)
    }
}

#[cfg(test)]
mod tests {
    use crate::precompiles::bls12_381::{
        ISTHMUS_G1_MSM_MAX_INPUT_SIZE, ISTHMUS_G2_MSM_MAX_INPUT_SIZE,
        ISTHMUS_PAIRING_MAX_INPUT_SIZE, JOVIAN_G1_MSM_MAX_INPUT_SIZE, JOVIAN_G2_MSM_MAX_INPUT_SIZE,
        JOVIAN_PAIRING_MAX_INPUT_SIZE, run_g1_msm_isthmus, run_g1_msm_jovian, run_g2_msm_isthmus,
        run_g2_msm_jovian,
    };

    use super::*;
    use revm::{
        precompile::{PrecompileHalt, PrecompileStatus, bls12_381_const},
        primitives::{Bytes, hex},
    };
    use std::vec;

    #[test]
    fn test_bn254_pair() {
        let input = hex::decode(
            "\
      1c76476f4def4bb94541d57ebba1193381ffa7aa76ada664dd31c16024c43f59\
      3034dd2920f673e204fee2811c678745fc819b55d3e9d294e45c9b03a76aef41\
      209dd15ebff5d46c4bd888e51a93cf99a7329636c63514396b4a452003a35bf7\
      04bf11ca01483bfa8b34b43561848d28905960114c8ac04049af4b6315a41678\
      2bb8324af6cfc93537a2ad1a445cfd0ca2a71acd7ac41fadbf933c2a51be344d\
      120a2a4cf30c1bf9845f20c6fe39e07ea2cce61f0c9bb048165fe5e4de877550\
      111e129f1cf1097710d41c4ac70fcdfa5ba2023c6ff1cbeac322de49d1b6df7c\
      2032c61a830e3c17286de9462bf242fca2883585b93870a73853face6a6bf411\
      198e9393920d483a7260bfb731fb5d25f1aa493335a9e71297e485b7aef312c2\
      1800deef121f1e76426a00665e5c4479674322d4f75edadd46debd5cd992f6ed\
      090689d0585ff075ec9e99ad690c3395bc4b313370b38ef355acdadcd122975b\
      12c85ea5db8c6deb4aab71808dcb408fe3d1e7690c43d37b4ce6cc0166fa7daa",
        )
        .unwrap();
        let expected =
            hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();
        let outcome = bn254_pair::run_pair_granite(&input, 260_000).unwrap();
        assert_eq!(outcome.bytes, expected);

        // Invalid input length
        let input = hex::decode(
            "\
          1111111111111111111111111111111111111111111111111111111111111111\
          1111111111111111111111111111111111111111111111111111111111111111\
          111111111111111111111111111111\
      ",
        )
        .unwrap();

        let res = bn254_pair::run_pair_granite(&input, 260_000);
        assert!(matches!(res, Err(PrecompileHalt::Bn254PairLength)));

        // Valid input length shorter than 112687
        let input = vec![1u8; 586 * bn254::PAIR_ELEMENT_LEN];
        let res = bn254_pair::run_pair_granite(&input, 260_000);
        assert!(matches!(res, Err(PrecompileHalt::OutOfGas)));

        // Input length longer than 112687
        let input = vec![1u8; 587 * bn254::PAIR_ELEMENT_LEN];
        let res = bn254_pair::run_pair_granite(&input, 260_000);
        assert!(matches!(res, Err(PrecompileHalt::Bn254PairLength)));
    }

    #[test]
    fn test_accelerated_bn254_pairing_jovian() {
        const TEST_INPUT: [u8; 384] = hex!(
            "2cf44499d5d27bb186308b7af7af02ac5bc9eeb6a3d147c186b21fb1b76e18da2c0f001f52110ccfe69108924926e45f0b0c868df0e7bde1fe16d3242dc715f61fb19bb476f6b9e44e2a32234da8212f61cd63919354bc06aef31e3cfaff3ebc22606845ff186793914e03e21df544c34ffe2f2f3504de8a79d9159eca2d98d92bd368e28381e8eccb5fa81fc26cf3f048eea9abfdd85d7ed3ab3698d63e4f902fe02e47887507adf0ff1743cbac6ba291e66f59be6bd763950bb16041a0a85e000000000000000000000000000000000000000000000000000000000000000130644e72e131a029b85045b68181585d97816a916871ca8d3c208c16d87cfd451971ff0471b09fa93caaf13cbf443c1aede09cc4328f5a62aad45f40ec133eb4091058a3141822985733cbdddfed0fd8d6c104e9e9eff40bf5abfef9ab163bc72a23af9a5ce2ba2796c1f4e453a370eb0af8c212d9dc9acd8fc02c2e907baea223a8eb0b0996252cb548a4487da97b02422ebc0e834613f954de6c7e0afdc1fc"
        );
        const EXPECTED_OUTPUT: [u8; 32] =
            hex!("0000000000000000000000000000000000000000000000000000000000000001");

        let res = bn254_pair::run_pair_jovian(TEST_INPUT.as_ref(), u64::MAX);
        assert!(matches!(res, Ok(outcome) if **outcome.bytes == EXPECTED_OUTPUT));
    }

    #[test]
    fn test_accelerated_bn254_pairing_bad_input_len_jovian() {
        let input = [0u8; bn254_pair::JOVIAN_MAX_INPUT_SIZE + 1];
        let res = bn254_pair::run_pair_jovian(&input, u64::MAX);
        assert!(matches!(res, Err(PrecompileHalt::Bn254PairLength)));
    }

    #[test]
    fn test_get_jovian_precompile_with_bad_input_len() {
        let precompiles = OpPrecompiles::new_with_spec(OpSpecId::JOVIAN);
        let bn254_pair_precompile = precompiles.precompiles().get(&bn254::pair::ADDRESS).unwrap();

        let mut bad_input_len = bn254_pair::JOVIAN_MAX_INPUT_SIZE + 1;
        assert!(bad_input_len < bn254_pair::GRANITE_MAX_INPUT_SIZE);
        let input = vec![0u8; bad_input_len];

        let res = bn254_pair_precompile.execute(&input, u64::MAX, 0).unwrap();
        assert!(matches!(res.status, PrecompileStatus::Halt(PrecompileHalt::Bn254PairLength)));

        let bls12_381_g1_msm_precompile =
            precompiles.precompiles().get(&bls12_381_const::G1_MSM_ADDRESS).unwrap();
        bad_input_len = bls12_381::JOVIAN_G1_MSM_MAX_INPUT_SIZE + 1;
        assert!(bad_input_len < bls12_381::ISTHMUS_G1_MSM_MAX_INPUT_SIZE);
        let input = vec![0u8; bad_input_len];
        let res = bls12_381_g1_msm_precompile.execute(&input, u64::MAX, 0).unwrap();
        assert!(
            matches!(res.status, PrecompileStatus::Halt(PrecompileHalt::Other(msg)) if msg.contains("input length too long"))
        );

        let bls12_381_g2_msm_precompile =
            precompiles.precompiles().get(&bls12_381_const::G2_MSM_ADDRESS).unwrap();
        bad_input_len = bls12_381::JOVIAN_G2_MSM_MAX_INPUT_SIZE + 1;
        assert!(bad_input_len < bls12_381::ISTHMUS_G2_MSM_MAX_INPUT_SIZE);
        let input = vec![0u8; bad_input_len];
        let res = bls12_381_g2_msm_precompile.execute(&input, u64::MAX, 0).unwrap();
        assert!(
            matches!(res.status, PrecompileStatus::Halt(PrecompileHalt::Other(msg)) if msg.contains("input length too long"))
        );

        let bls12_381_pairing_precompile =
            precompiles.precompiles().get(&bls12_381_const::PAIRING_ADDRESS).unwrap();
        bad_input_len = bls12_381::JOVIAN_PAIRING_MAX_INPUT_SIZE + 1;
        assert!(bad_input_len < bls12_381::ISTHMUS_PAIRING_MAX_INPUT_SIZE);
        let input = vec![0u8; bad_input_len];
        let res = bls12_381_pairing_precompile.execute(&input, u64::MAX, 0).unwrap();
        assert!(
            matches!(res.status, PrecompileStatus::Halt(PrecompileHalt::Other(msg)) if msg.contains("input length too long"))
        );
    }

    #[test]
    fn test_cancun_precompiles_in_fjord() {
        // additional to cancun, fjord has p256verify
        assert_eq!(fjord().difference(Precompiles::cancun()).len(), 1)
    }

    #[test]
    fn test_cancun_precompiles_in_granite() {
        // granite has p256verify (fjord)
        // granite has modification of cancun's bn254 pair (doesn't count as new precompile)
        assert_eq!(granite().difference(Precompiles::cancun()).len(), 1)
    }

    #[test]
    fn test_prague_precompiles_in_isthmus() {
        let new_prague_precompiles = Precompiles::prague().difference(Precompiles::cancun());

        // isthmus contains all precompiles that were new in prague, without modifications
        assert!(new_prague_precompiles.difference(isthmus()).is_empty())
    }

    #[test]
    fn test_prague_precompiles_in_jovian() {
        let new_prague_precompiles = Precompiles::prague().difference(Precompiles::cancun());

        // jovian contains all precompiles that were new in prague, without modifications
        assert!(new_prague_precompiles.difference(jovian()).is_empty())
    }

    #[test]
    fn test_osaka_precompiles_in_karst() {
        let new_osaka_precompiles = Precompiles::osaka().difference(Precompiles::prague());

        // karst contains all precompiles that were new in karst, without modifications
        assert!(new_osaka_precompiles.difference(karst()).is_empty())
    }

    /// All the addresses of the precompiles in isthmus should be in jovian
    #[test]
    fn test_isthmus_precompiles_in_jovian() {
        let new_isthmus_precompiles = isthmus().difference(Precompiles::cancun());

        // jovian contains all precompiles that were new in isthmus, without modifications
        assert!(new_isthmus_precompiles.difference(jovian()).is_empty())
    }

    #[test]
    fn test_default_precompiles_is_latest() {
        let latest = OpPrecompiles::new_with_spec(OpSpecId::default()).inner.precompiles;
        let default = OpPrecompiles::default().inner.precompiles;
        assert_eq!(latest.len(), default.len());

        let intersection = default.intersection(latest);
        assert_eq!(intersection.len(), latest.len())
    }

    #[test]
    fn test_g1_isthmus_max_size() {
        let oversized_input = vec![0u8; ISTHMUS_G1_MSM_MAX_INPUT_SIZE + 1];
        let input = Bytes::from(oversized_input);

        let res = run_g1_msm_isthmus(&input, 260_000);

        assert!(
            matches!(res, Err(PrecompileHalt::Other(msg)) if msg.contains("input length too long"))
        );
    }

    #[test]
    fn test_g1_jovian_max_size() {
        let oversized_input = vec![0u8; JOVIAN_G1_MSM_MAX_INPUT_SIZE + 1];
        let input = Bytes::from(oversized_input);

        let res = run_g1_msm_jovian(&input, u64::MAX);

        assert!(
            matches!(res, Err(PrecompileHalt::Other(msg)) if msg.contains("input length too long"))
        );
    }
    #[test]
    fn test_g2_isthmus_max_size() {
        let oversized_input = vec![0u8; ISTHMUS_G2_MSM_MAX_INPUT_SIZE + 1];
        let input = Bytes::from(oversized_input);

        let res = run_g2_msm_isthmus(&input, 260_000);

        assert!(
            matches!(res, Err(PrecompileHalt::Other(msg)) if msg.contains("input length too long"))
        );
    }
    #[test]
    fn test_g2_jovian_max_size() {
        let oversized_input = vec![0u8; JOVIAN_G2_MSM_MAX_INPUT_SIZE + 1];
        let input = Bytes::from(oversized_input);

        let res = run_g2_msm_jovian(&input, u64::MAX);

        assert!(matches!(
            res,
            Err(PrecompileHalt::Other(msg)) if msg.contains("input length too long")));
    }
    #[test]
    fn test_pair_isthmus_max_size() {
        let oversized_input = vec![0u8; ISTHMUS_PAIRING_MAX_INPUT_SIZE + 1];
        let input = Bytes::from(oversized_input);

        let res = bls12_381::run_pair_isthmus(&input, 260_000);

        assert!(
            matches!(res, Err(PrecompileHalt::Other(msg)) if msg.contains("input length too long"))
        );
    }
    #[test]
    fn test_pair_jovian_max_size() {
        let oversized_input = vec![0u8; JOVIAN_PAIRING_MAX_INPUT_SIZE + 1];
        let input = Bytes::from(oversized_input);

        let res = bls12_381::run_pair_jovian(&input, u64::MAX);

        assert!(
            matches!(res, Err(PrecompileHalt::Other(msg)) if msg.contains("input length too long"))
        );
    }
}
