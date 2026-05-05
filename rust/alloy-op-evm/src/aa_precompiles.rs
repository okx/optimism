//! AA (EIP-8130) system precompile registration.
//!
//! Wires the `NonceManager` (`0x…aa02`) and `TxContext` (`0x…aa03`)
//! precompiles into the [`PrecompilesMap`] used by [`OpEvmFactory`].
//!
//! Without this module, the static `OpPrecompiles::precompiles()` set
//! does NOT include the EIP-8130 precompiles (they live in op-revm's
//! `<OpPrecompiles as PrecompileProvider>::run` interceptor that
//! `PrecompilesMap::from_static(...)` strips away). Phase calls to
//! either address would fall through to the stub bytecode `0xfe` and
//! revert.
//!
//! Mirrors `reth-projects/base @ crates/common/evm/src/factory.rs`.
//! Each closure reads from op-revm's existing thread-local
//! `EIP8130_TX_CONTEXT` (set/cleared by `handler.rs`) so the dispatch
//! is context-aware without needing extra plumbing.
//!
//! Note: revm-precompile 34's `PrecompileError` only has `Fatal{,Any}`
//! variants — those halt the EVM. For "input invalid" / "unknown selector"
//! we return `Ok(PrecompileOutput::revert(...))` instead, matching how
//! a contract revert would look from the EVM's perspective.

use alloy_evm::precompiles::{DynPrecompile, PrecompileInput, PrecompilesMap};
use alloy_primitives::{Address, Bytes, U256};
use op_revm::{
    OpSpecId,
    precompiles::{
        NONCE_MANAGER_ADDRESS, NONCE_MANAGER_GAS, OpPrecompiles, TX_CONTEXT_ADDRESS,
        TX_CONTEXT_GAS, aa_nonce_slot, encode_address, encode_b256, encode_calls_abi,
        encode_u256, get_eip8130_tx_context, selector,
    },
};
use revm::precompile::{
    PrecompileHalt, PrecompileId, PrecompileOutput, PrecompileResult,
};

/// Builds a "soft" revert output. Used for invalid-input paths so the
/// caller frame sees a regular revert instead of an EVM-fatal error.
fn revert(input: &PrecompileInput<'_>) -> PrecompileResult {
    Ok(PrecompileOutput::revert(0, Bytes::new(), input.reservoir))
}

/// Builds an out-of-gas halt output.
fn out_of_gas(input: &PrecompileInput<'_>) -> PrecompileResult {
    Ok(PrecompileOutput::halt(PrecompileHalt::OutOfGas, input.reservoir))
}

fn make_tx_context_precompile() -> DynPrecompile {
    DynPrecompile::new_stateful(PrecompileId::custom("tx_context"), |input| {
        if input.data.len() < 4 {
            return revert(&input);
        }

        let ctx = get_eip8130_tx_context();
        let (sender, payer, owner_id, gas_limit, max_cost) = match &ctx {
            Some(c) => (c.sender, c.payer, c.owner_id.0, c.gas_limit, c.max_cost),
            None => (Address::ZERO, Address::ZERO, [0u8; 32], 0, U256::ZERO),
        };

        let sel = &input.data[0..4];
        let output = if sel == selector(b"getSender()") {
            encode_address(sender)
        } else if sel == selector(b"getPayer()") {
            encode_address(payer)
        } else if sel == selector(b"getOwnerId()") {
            encode_b256(owner_id)
        } else if sel == selector(b"getMaxCost()") {
            encode_u256(max_cost)
        } else if sel == selector(b"getGasLimit()") {
            encode_u256(U256::from(gas_limit))
        } else if sel == selector(b"getCalls()") {
            let phases = ctx.as_ref().map(|c| &c.call_phases[..]).unwrap_or(&[]);
            encode_calls_abi(phases)
        } else {
            return revert(&input);
        };

        if input.gas < TX_CONTEXT_GAS {
            return out_of_gas(&input);
        }
        Ok(PrecompileOutput::new(TX_CONTEXT_GAS, output, input.reservoir))
    })
}

fn make_nonce_manager_precompile() -> DynPrecompile {
    DynPrecompile::new_stateful(PrecompileId::custom("nonce_manager"), |mut input| {
        let get_nonce_sel = selector(b"getNonce(address,uint256)");

        if input.data.len() < 4 || input.data[0..4] != get_nonce_sel {
            return revert(&input);
        }
        if input.data.len() < 4 + 32 + 32 {
            return revert(&input);
        }

        let account = Address::from_slice(&input.data[4 + 12..4 + 32]);
        let nonce_key = U256::from_be_slice(&input.data[4 + 32..4 + 64]);
        let slot = aa_nonce_slot(account, nonce_key);

        // sload errors → soft revert (DB read failure is unusual but recoverable
        // from the caller's perspective — let the frame see a revert).
        let storage_value =
            match input.internals.sload(NONCE_MANAGER_ADDRESS, slot) {
                Ok(v) => v,
                Err(_) => return revert(&input),
            };

        let mut out = [0u8; 32];
        let storage_bytes = storage_value.data.to_be_bytes::<32>();
        out[24..32].copy_from_slice(&storage_bytes[24..32]);

        if input.gas < NONCE_MANAGER_GAS {
            return out_of_gas(&input);
        }
        Ok(PrecompileOutput::new(
            NONCE_MANAGER_GAS,
            Bytes::from(out.to_vec()),
            input.reservoir,
        ))
    })
}

/// Builds a [`PrecompilesMap`] for the given spec, including the EIP-8130
/// system precompiles when `NATIVE_AA` (or later) is active.
///
/// At earlier specs the inner static set from `OpPrecompiles` is used as-is,
/// so legacy precompile dispatch is unaffected.
pub fn op_precompiles_map(spec: OpSpecId) -> PrecompilesMap {
    let mut map =
        PrecompilesMap::from_static(OpPrecompiles::new_with_spec(spec).precompiles());

    if spec == OpSpecId::NATIVE_AA {
        map.extend_precompiles([
            (TX_CONTEXT_ADDRESS, make_tx_context_precompile()),
            (NONCE_MANAGER_ADDRESS, make_nonce_manager_precompile()),
        ]);
    }

    map
}
