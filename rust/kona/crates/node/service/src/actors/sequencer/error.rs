use crate::{
    L1OriginSelectorError, UnsafePayloadGossipClientError, actors::engine::EngineClientError,
};
use kona_derive::PipelineErrorKind;
use kona_engine::BuildTaskError;

/// An error produced by the [`crate::SequencerActor`].
#[derive(Debug, thiserror::Error)]
pub enum SequencerActorError {
    /// An error occurred while building payload attributes.
    #[error(transparent)]
    AttributesBuilder(#[from] PipelineErrorKind),
    /// A channel was unexpectedly closed.
    #[error("Channel closed unexpectedly")]
    ChannelClosed,
    /// An error occurred while selecting the next L1 origin.
    #[error(transparent)]
    L1OriginSelector(#[from] L1OriginSelectorError),
    /// An error occurred communicating with the engine.
    #[error(transparent)]
    EngineError(#[from] EngineClientError),
    /// An error occurred while attempting to build a payload.
    #[error(transparent)]
    BuildError(#[from] BuildTaskError),
    /// An error occurred while attempting to schedule unsafe payload gossip.
    #[error("An error occurred while attempting to schedule unsafe payload gossip: {0}")]
    PayloadGossip(#[from] UnsafePayloadGossipClientError),
    /// Failed to convert a just-sealed payload into the local block representation.
    /// Surfaced rather than silently swallowed because the steady-state path uses the
    /// derived block to (a) compute the parent of the next build (avoiding a watch-channel
    /// race against the engine actor) and (b) prime the L2 [`kona_genesis::SystemConfig`]
    /// cache (required because the canonicalize forkchoice update is deferred — without
    /// the prime, `prepare_payload_attributes` would call
    /// `eth_getBlockByNumber(parent.number)` and get null until the next FCU).
    #[error("Failed to convert sealed payload into block: {0}")]
    SealedPayloadConversion(String),
}
