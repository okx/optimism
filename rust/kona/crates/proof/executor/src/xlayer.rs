use alloy_evm::EvmFactory;
use alloy_op_evm::XLayerGaslessFeeHookFactory;

/// Combined factory bound for all XLayer OP proof executors.
///
/// Any type that implements both [`EvmFactory`] and [`XLayerGaslessFeeHookFactory`]
/// automatically satisfies this trait via the blanket impl below. Using this trait
/// as a single bound avoids repeating `EvmFactory + XLayerGaslessFeeHookFactory`
/// throughout kona's generic signatures.
pub trait XLayerEvmFactory: EvmFactory + XLayerGaslessFeeHookFactory {}

impl<T: EvmFactory + XLayerGaslessFeeHookFactory> XLayerEvmFactory for T {}
