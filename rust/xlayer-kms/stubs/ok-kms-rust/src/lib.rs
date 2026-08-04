//! Local stand-in for the private `ok-kms-rust` SDK
//! (`ssh://git@gitlab.okg.com/okcoin-commons/ok-kms-rust.git`).
//!
//! Its only job is to make `xlayer-kms` resolvable and compilable — including
//! under `--features kms` — without access to the private `GitLab` group. That
//! keeps `cargo check`, `clippy --workspace --all-features` and `cargo test`
//! working for everyone, since Cargo resolves dependencies regardless of which
//! features are enabled.
//!
//! It deliberately resolves nothing. [`KmsClient::get_value_by_key`] always
//! fails rather than returning a placeholder secret, so a binary accidentally
//! built with `--features kms` against this stub cannot silently authenticate
//! with a bogus credential — it fails loudly on first use instead.
//!
//! `just kms-crate` replaces this directory with a real checkout of the SDK for
//! production builds. To tell which one a binary was linked against, grep it for
//! the marker below (mirrors the `KMS support is not compiled into this binary`
//! check used on the Go side):
//!
//! ```text
//! grep -ac OK_KMS_STUB target/release/kona-node   # 0 = real SDK linked
//! ```

/// Marker embedded in [`StubError`]'s message so a built binary can be checked
/// for the stub. Must not appear in the real SDK.
pub const STUB_MARKER: &str = "OK_KMS_STUB";

/// The error type returned by every stub operation.
#[derive(Debug)]
pub struct StubError(String);

impl core::fmt::Display for StubError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl core::error::Error for StubError {}

/// Stand-in for the SDK's KMS client. Constructs successfully so that callers'
/// initialization paths stay exercised; every lookup then fails.
#[derive(Debug, Default)]
pub struct KmsClient;

impl KmsClient {
    /// Creates a stub client. Never fails.
    pub const fn new() -> Result<Self, StubError> {
        Ok(Self)
    }

    /// Always fails: this stub has no KMS backend.
    pub fn get_value_by_key(&self, key: &str) -> Result<String, StubError> {
        Err(StubError(format!(
            "{STUB_MARKER}: the real ok-kms-rust SDK is not linked into this binary, so kms key {key:?} cannot be resolved; rebuild after `just kms-crate`"
        )))
    }
}
