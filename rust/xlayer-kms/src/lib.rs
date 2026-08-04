//! X Layer: optional KMS-backed secret resolution.
//!
//! Secrets can live in the company KMS and be referenced by a `kms:<name>`
//! prefix — either supplied directly as a flag value (e.g.
//! `--l2.jwt-secret kms:rollup-jwt`) or as the contents of a secret file the
//! flag points at (e.g. a mounted file containing `kms:rpcnode-p2p-key`). At
//! load time the referenced key is fetched from the KMS (already decrypted by
//! the SDK) and its plaintext is used in place of the reference. Values without
//! the prefix pass through unchanged, so non-KMS deployments behave exactly as
//! upstream.
//!
//! Reference detection ([`is_kms_ref`]) and resolution ([`maybe_resolve`])
//! always compile. Only `get_secret_value` — the single function that talks to
//! the private `ok-kms-rust` SDK — is gated by the `kms` cargo feature: with the
//! feature it calls the SDK, without it the SDK is never referenced and every
//! reference resolves to [`KmsError::Disabled`]. This mirrors op-node's
//! `client_kms.go` / `client_nokms.go` split, but Rust's line-level `#[cfg]`
//! lets both variants live in one file.
//!
//! # Why this is its own crate
//!
//! The private SDK lives in a `GitLab` group most people cannot reach, and Cargo
//! resolves dependencies without regard for features — `optional = true` decides
//! whether a crate is *compiled*, not whether it is *resolved*. Declaring the
//! SDK anywhere in the workspace as a `git` dependency therefore breaks every
//! `cargo check`/`clippy`/`test` for anyone without gitlab.okg.com access,
//! feature flags notwithstanding.
//!
//! Keeping it here confines that problem to one X Layer-owned manifest (rather
//! than an upstream kona crate) and pairs it with `stubs/ok-kms-rust`, an
//! in-tree stand-in the committed manifest points at so nothing needs the
//! network. `just kms-crate` swaps the stub for a real checkout before a
//! production build. Consumers depend on this crate directly from their binary
//! crate and forward a `kms` feature; see kona-node's `main.rs` (and
//! xlayer-reth's), which resolve references by rewriting argv/env before clap
//! parses, so no library crate is involved.

use thiserror::Error;

/// The prefix that marks a value as a KMS key reference rather than a literal
/// secret.
pub const KMS_REF_PREFIX: &str = "kms:";

/// Errors returned while resolving a `kms:<name>` reference.
#[derive(Debug, Error)]
pub enum KmsError {
    /// A `kms:` reference was used but KMS support was not compiled into the
    /// binary.
    #[error(
        "KMS support is not compiled into this binary: rebuild with the `kms` cargo feature (e.g. `cargo build --features kms`) to resolve kms:<name> references"
    )]
    Disabled,
    /// The referenced key resolved to an empty value.
    #[error("kms key {0:?} resolved to an empty value")]
    Empty(String),
    /// The KMS backend returned an error.
    #[error("kms backend error: {0}")]
    Backend(String),
}

/// Returns `true` if `value` (after trimming surrounding whitespace) is a
/// `kms:<name>` reference.
pub fn is_kms_ref(value: &str) -> bool {
    value.trim().starts_with(KMS_REF_PREFIX)
}

/// Resolves a `kms:<name>` reference to its plaintext secret.
///
/// Values that are not KMS references are returned unchanged, so literal hex
/// secrets and plaintext file contents behave exactly as before. A reference is
/// resolved via the KMS backend; without the `kms` feature this returns
/// [`KmsError::Disabled`].
///
/// Callers that accept a file path should check [`is_kms_ref`] on the raw
/// argument *before* touching the filesystem, so a direct `kms:<name>` value is
/// resolved via the KMS rather than mistaken for a (non-existent) file path.
///
/// # Errors
///
/// - [`KmsError::Disabled`] if a `kms:` reference is used without the `kms` feature compiled in.
/// - [`KmsError::Empty`] if the referenced key resolves to an empty value.
/// - [`KmsError::Backend`] if the KMS backend fails to fetch the key.
pub fn maybe_resolve(value: &str) -> Result<String, KmsError> {
    if !is_kms_ref(value) {
        // Preserve the caller's exact value (including any surrounding
        // whitespace) so non-KMS parsing is byte-for-byte unchanged.
        return Ok(value.to_string());
    }

    let key = value.trim().strip_prefix(KMS_REF_PREFIX).unwrap_or_default().trim();
    let secret = get_secret_value(key)?;
    if secret.is_empty() {
        return Err(KmsError::Empty(key.to_string()));
    }
    Ok(secret)
}

/// X Layer: production KMS lookup, compiled only with the `kms` feature.
///
/// This is the ONLY place in the workspace that references the private
/// `ok-kms-rust` crate (`git@gitlab.okg.com:okcoin-commons/ok-kms-rust`). The
/// SDK reads `KMS_ENABLED` from the environment, writes out and loads its
/// embedded `kms_linux.so`, and fetches already-decrypted secrets by key name.
///
/// The client is constructed AT MOST ONCE per process and never dropped.
#[cfg(feature = "kms")]
fn get_secret_value(key: &str) -> Result<String, KmsError> {
    use ok_kms_rust::KmsClient;
    use std::sync::OnceLock;

    /// Wrapper asserting the SDK client is safe to share as a process-wide
    /// static.
    struct ProcessClient(KmsClient);
    unsafe impl Send for ProcessClient {}
    unsafe impl Sync for ProcessClient {}

    // Never dropped (statics don't run destructors), so the library is never
    // dlclose'd and the Go runtime inside it stays valid for the process
    // lifetime.
    static CLIENT: OnceLock<Result<ProcessClient, String>> = OnceLock::new();

    let client = CLIENT
        .get_or_init(|| KmsClient::new().map(ProcessClient).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| KmsError::Backend(e.clone()))?;
    client.0.get_value_by_key(key).map_err(|e| KmsError::Backend(e.to_string()))
}

/// Stub compiled when the `kms` feature is disabled: the private `ok-kms-rust`
/// crate is never referenced, so the default build needs no access to the
/// private `GitLab` group.
#[cfg(not(feature = "kms"))]
const fn get_secret_value(_key: &str) -> Result<String, KmsError> {
    Err(KmsError::Disabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_kms_ref_detects_prefix() {
        assert!(is_kms_ref("kms:my-key"));
        assert!(is_kms_ref("  kms:my-key  "));
        assert!(!is_kms_ref("0xdeadbeef"));
        assert!(!is_kms_ref("not-a-kms-ref"));
        assert!(!is_kms_ref(""));
    }

    #[test]
    fn maybe_resolve_passes_through_literals_unchanged() {
        // Non-references must be returned byte-for-byte, regardless of feature.
        let literal = "  0xdeadbeef\n";
        assert_eq!(maybe_resolve(literal).unwrap(), literal);
    }

    #[cfg(not(feature = "kms"))]
    #[test]
    fn maybe_resolve_rejects_refs_without_feature() {
        let err = maybe_resolve("kms:some-key").unwrap_err();
        assert!(matches!(err, KmsError::Disabled));
    }

    /// With the `kms` feature the stub client constructs successfully and then
    /// fails every lookup. Repeated resolution must keep reaching the backend
    /// (i.e. reuse the one process-wide client) rather than caching a per-key
    /// lookup failure as if construction itself had failed.
    #[cfg(feature = "kms")]
    #[test]
    fn maybe_resolve_reuses_the_process_client_across_lookups() {
        for key in ["kms:first-key", "kms:second-key"] {
            let err = maybe_resolve(key).unwrap_err();
            assert!(
                matches!(&err, KmsError::Backend(msg) if msg.contains(ok_kms_rust::STUB_MARKER)),
                "expected a stub backend error for {key}, got: {err}"
            );
        }
    }
}
