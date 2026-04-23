use reth_payload_builder::PayloadId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChannelEngineError {
    #[error("new_payload failed: {0}")]
    NewPayload(String),
    #[error("fork_choice_updated failed: {0}")]
    Fcu(String),
    #[error("payload not found for id {0:?}")]
    PayloadNotFound(PayloadId),
    #[error("payload builder error: {0}")]
    PayloadBuilder(String),
}
