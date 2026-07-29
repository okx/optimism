#![doc = include_str!("../README.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/ethereum-optimism/optimism/develop/rust/kona/assets/square.png",
    html_favicon_url = "https://raw.githubusercontent.com/ethereum-optimism/optimism/develop/rust/kona/assets/favicon.ico",
    issue_tracker_base_url = "https://github.com/ethereum-optimism/optimism/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod cli;
pub mod commands;
pub mod flags;
pub mod metrics;

pub(crate) mod version;

fn main() {
    use clap::Parser;

    kona_cli::sigsegv_handler::install();
    kona_cli::backtrace::enable();

    // X Layer: swap `kms:<name>` references in secret-bearing flags for their
    // plaintext BEFORE clap parses. This is the single KMS injection point —
    // every secret has an upstream sibling flag that carries the raw value, so
    // rewriting argv/env here means no kona crate is involved in resolution.
    let argv = match resolve_kms_secret_flags(std::env::args().collect()) {
        Ok(argv) => argv,
        Err(err) => {
            eprintln!("X Layer KMS configuration error: {err}");
            std::process::exit(1);
        }
    };

    if let Err(err) = cli::Cli::parse_from(argv).run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

/// X Layer: one secret-bearing, file-path-taking flag whose value (or the
/// contents of the file it names) may be a `kms:<name>` reference, together
/// with the upstream sibling flag that accepts the secret itself.
struct KmsRewrite {
    /// Long names accepting the file path: the primary name plus any visible
    /// aliases, all of which clap accepts and so all of which must be scanned.
    path_flags: &'static [&'static str],
    /// The env fallback of the path flag.
    path_env: &'static str,
    /// The sibling flag carrying the raw secret in memory.
    raw_flag: &'static str,
    /// The env fallback of the sibling flag.
    raw_env: &'static str,
    /// What the secret is, for error messages.
    what: &'static str,
}

/// The three kona-node secrets that may live in the KMS. Each rewrite moves a
/// resolved reference onto the sibling flag, so the secret stays in memory and
/// upstream code only ever sees the plaintext carrier it already supports.
const KMS_REWRITES: &[KmsRewrite] = &[
    KmsRewrite {
        path_flags: &["--l2-engine-jwt-secret", "--l2.jwt-secret"],
        path_env: "KONA_NODE_L2_ENGINE_AUTH",
        raw_flag: "--l2-engine-jwt-encoded",
        raw_env: "KONA_NODE_L2_ENGINE_AUTH_ENCODED",
        what: "L2 engine JWT secret",
    },
    KmsRewrite {
        path_flags: &["--p2p.priv.path"],
        path_env: "KONA_NODE_P2P_PRIV_PATH",
        raw_flag: "--p2p.priv.raw",
        raw_env: "KONA_NODE_P2P_PRIV_RAW",
        what: "p2p private key",
    },
    KmsRewrite {
        path_flags: &["--p2p.sequencer.key.path"],
        path_env: "KONA_NODE_P2P_SEQUENCER_KEY_PATH",
        raw_flag: "--p2p.sequencer.key",
        raw_env: "KONA_NODE_P2P_SEQUENCER_KEY",
        what: "sequencer key",
    },
];

/// X Layer: resolves `kms:<name>` references in secret-bearing flags (and their
/// environment fallbacks) before clap ever sees them. A reference may be given
/// directly as the flag value, or as the sole content of the file the flag
/// points at. Anything that is not a reference — plain key files, paths that do
/// not exist yet — passes through byte-for-byte, so non-KMS deployments behave
/// exactly as upstream. A reference that fails to resolve (no `kms` build
/// feature, KMS unreachable, malformed secret) is a hard error: booting a node
/// with a wrong identity or JWT is worse than not booting.
fn resolve_kms_secret_flags(mut argv: Vec<String>) -> anyhow::Result<Vec<String>> {
    for rewrite in KMS_REWRITES {
        apply_kms_rewrite(&mut argv, rewrite)?;
    }
    Ok(argv)
}

/// Applies one [`KmsRewrite`] to argv and the environment.
fn apply_kms_rewrite(argv: &mut [String], rw: &KmsRewrite) -> anyhow::Result<()> {
    // If the raw sibling is already set (flag or env), upstream either prefers
    // it over the path variant or reports the conflict; rewriting would invert
    // that precedence, so leave everything untouched.
    if flag_is_present(argv, rw.raw_flag) || std::env::var_os(rw.raw_env).is_some() {
        return Ok(());
    }

    let mut rewrote = false;
    for flag in rw.path_flags {
        rewrote |= rewrite_flag_occurrences(argv, flag, rw)?;
    }
    if rewrote {
        // The flag masked this env fallback; clear it so clap does not fill the
        // now-absent path flag back in from the environment (which would also
        // trip the sequencer pair's conflicts_with against the renamed flag).
        remove_env(rw.path_env);
        return Ok(());
    }

    // Path flag absent from argv: clap would fall back to the env var.
    if let Ok(val) = std::env::var(rw.path_env) &&
        let Some(reference) = kms_ref_in_value_or_file(&val)?
    {
        let secret = resolve_kms_secret(&reference, rw.what)?;
        remove_env(rw.path_env);
        set_env(rw.raw_env, &secret);
    }
    Ok(())
}

/// Rewrites every `--flag value` / `--flag=value` occurrence whose value is (or
/// whose file contains) a `kms:` reference into `raw_flag` carrying the
/// resolved secret. Returns whether anything was rewritten. Scanning is
/// positional and stops at a literal `--`, after which clap treats everything
/// as positional arguments; flags that merely share the prefix never match.
fn rewrite_flag_occurrences(
    argv: &mut [String],
    flag: &str,
    rw: &KmsRewrite,
) -> anyhow::Result<bool> {
    let mut rewrote = false;
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "--" {
            break;
        }
        if argv[i] == flag {
            if let Some(val) = argv.get(i + 1).cloned() &&
                let Some(reference) = kms_ref_in_value_or_file(&val)?
            {
                argv[i] = rw.raw_flag.to_string();
                argv[i + 1] = resolve_kms_secret(&reference, rw.what)?;
                rewrote = true;
            }
            i += 2;
        } else {
            let inline_val = argv[i]
                .strip_prefix(flag)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::to_string);
            if let Some(val) = inline_val &&
                let Some(reference) = kms_ref_in_value_or_file(&val)?
            {
                let secret = resolve_kms_secret(&reference, rw.what)?;
                argv[i] = format!("{}={secret}", rw.raw_flag);
                rewrote = true;
            }
            i += 1;
        }
    }
    Ok(rewrote)
}

/// Returns whether `flag` appears in argv (space or `=` form), up to a literal
/// `--`.
fn flag_is_present(argv: &[String], flag: &str) -> bool {
    argv.iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == flag || arg.strip_prefix(flag).is_some_and(|rest| rest.starts_with('=')))
}

/// Extracts a `kms:<name>` reference from a flag value that is nominally a file
/// path: either the value itself is a reference, or it names an existing file
/// whose contents are one. Returns `None` for everything else (plain key files,
/// paths that don't exist yet) so those keep their upstream behavior.
fn kms_ref_in_value_or_file(val: &str) -> anyhow::Result<Option<String>> {
    if xlayer_kms::is_kms_ref(val) {
        return Ok(Some(val.to_string()));
    }
    let path = std::path::Path::new(val);
    if path.exists() {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read secret key file {val}: {e}"))?;
        if xlayer_kms::is_kms_ref(&contents) {
            return Ok(Some(contents.trim().to_string()));
        }
    }
    Ok(None)
}

/// Resolves a reference and validates the plaintext as 32 hex-encoded bytes —
/// the shape of all three secrets — so a malformed KMS entry is attributed to
/// the KMS here rather than surfacing as a confusing clap error on a sibling
/// flag nobody passed.
fn resolve_kms_secret(reference: &str, what: &str) -> anyhow::Result<String> {
    use std::str::FromStr as _;
    let plain = xlayer_kms::maybe_resolve(reference).map_err(|e| anyhow::anyhow!("{what}: {e}"))?;
    let hex = plain.trim();
    alloy_primitives::B256::from_str(hex)
        .map_err(|e| anyhow::anyhow!("{what} from KMS is not 32 hex-encoded bytes: {e}"))?;
    Ok(hex.to_string())
}

/// SAFETY wrapper: only called from `main` before any other thread exists.
fn set_env(var: &str, val: &str) {
    // SAFETY: see above; single-threaded at this point.
    unsafe { std::env::set_var(var, val) };
}

/// SAFETY wrapper: only called from `main` before any other thread exists.
fn remove_env(var: &str) {
    // SAFETY: see above; single-threaded at this point.
    unsafe { std::env::remove_var(var) };
}

#[cfg(test)]
mod kms_flag_tests {
    use super::*;

    fn to_vec(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    /// A rewrite table entry with test-only env names, so tests can exercise
    /// the env path without touching real `KONA_NODE_*` variables.
    const TEST_RW: KmsRewrite = KmsRewrite {
        path_flags: &["--key.path", "--key-path-alias"],
        path_env: "XLAYER_KMS_TEST_KEY_PATH",
        raw_flag: "--key.raw",
        raw_env: "XLAYER_KMS_TEST_KEY_RAW",
        what: "test key",
    };

    #[test]
    fn plain_values_pass_through_untouched() {
        // Not references, not existing files: argv must stay byte-for-byte.
        let argv = to_vec(&["kona-node", "node", "--key.path", "/nonexistent", "--key.path="]);
        let mut rewritten = argv.clone();
        apply_kms_rewrite(&mut rewritten, &TEST_RW).unwrap();
        assert_eq!(rewritten, argv);
    }

    #[cfg(not(feature = "kms"))]
    #[test]
    fn references_fail_fast_without_the_kms_feature() {
        let mut argv = to_vec(&["kona-node", "node", "--key.path", "kms:some-key"]);
        let err = apply_kms_rewrite(&mut argv, &TEST_RW).unwrap_err();
        assert!(err.to_string().contains("KMS support is not compiled"), "{err}");
    }

    #[cfg(not(feature = "kms"))]
    #[test]
    fn aliases_are_scanned_too() {
        let mut argv = to_vec(&["kona-node", "node", "--key-path-alias=kms:some-key"]);
        let err = apply_kms_rewrite(&mut argv, &TEST_RW).unwrap_err();
        assert!(err.to_string().contains("test key"), "{err}");
    }

    #[cfg(not(feature = "kms"))]
    #[test]
    fn present_raw_sibling_skips_the_rewrite_entirely() {
        // Upstream precedence: the raw flag wins over the path flag, so a
        // reference in the path must stay untouched (and unresolved).
        let argv = to_vec(&["kona-node", "node", "--key.raw", "aa", "--key.path", "kms:some-key"]);
        let mut rewritten = argv.clone();
        apply_kms_rewrite(&mut rewritten, &TEST_RW).unwrap();
        assert_eq!(rewritten, argv);
    }

    #[cfg(not(feature = "kms"))]
    #[test]
    fn scanning_stops_at_the_positional_separator() {
        let argv = to_vec(&["kona-node", "node", "--", "--key.path", "kms:some-key"]);
        let mut rewritten = argv.clone();
        apply_kms_rewrite(&mut rewritten, &TEST_RW).unwrap();
        assert_eq!(rewritten, argv);
    }

    #[cfg(not(feature = "kms"))]
    #[test]
    fn references_inside_key_files_are_found() {
        let dir = std::env::temp_dir().join(format!("kona-kms-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ref-key");
        std::fs::write(&file, "kms:from-file\n").unwrap();

        let mut argv = to_vec(&["kona-node", "node", "--key.path", file.to_str().unwrap()]);
        let err = apply_kms_rewrite(&mut argv, &TEST_RW).unwrap_err();
        assert!(err.to_string().contains("KMS support is not compiled"), "{err}");

        // A file holding a plain hex key keeps its upstream path.
        std::fs::write(&file, "aa".repeat(32)).unwrap();
        let argv = to_vec(&["kona-node", "node", "--key.path", file.to_str().unwrap()]);
        let mut rewritten = argv.clone();
        apply_kms_rewrite(&mut rewritten, &TEST_RW).unwrap();
        assert_eq!(rewritten, argv);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(not(feature = "kms"))]
    #[test]
    fn env_fallback_is_rewritten_when_the_flag_is_absent() {
        // Uses the test-only env names from TEST_RW, so no real variable is
        // involved even when tests run in parallel.
        set_env(TEST_RW.path_env, "kms:from-env");
        let mut argv = to_vec(&["kona-node", "node"]);
        let err = apply_kms_rewrite(&mut argv, &TEST_RW).unwrap_err();
        remove_env(TEST_RW.path_env);
        assert!(err.to_string().contains("KMS support is not compiled"), "{err}");
    }
}
