//! X Layer L2 time fork logic.
//!
//! Mirrors `op-node/rollup/l2_time_xlayer.go` and the auto-fix logic in
//! `op-node/rollup/config_xlayer.go::FixXLayerL2Time` from okx/optimism PR
//! #178. Both X Layer mainnet and testnet shipped with the wrong
//! `Genesis.L2Time` in `rollup.json`; this module hardcodes the corrected
//! values, the activation timestamps at which span batch derivation must
//! switch from the old (broken) value to the new (correct) one, and the
//! auto-fix routine that overwrites the loaded config and the file on disk.
//!
//! For any chain ID other than X Layer mainnet/testnet this module is a
//! no-op: [`get_batch_start_time`] returns the standard
//! `genesis_timestamp + rel_timestamp` and [`fix_xlayer_l2_time`] returns
//! without touching the config or the file.

/// X Layer mainnet chain ID.
pub const XLAYER_MAINNET_CHAIN_ID: u64 = 196;
/// X Layer testnet (Sepolia) chain ID.
pub const XLAYER_TESTNET_CHAIN_ID: u64 = 1952;

/// Mainnet activation: 2025-12-08 14:00:00 UTC+8.
pub const MAINNET_L2_TIME_FORK_TIME: u64 = 1_765_173_600;
/// Testnet activation: 2025-12-04 18:00:00 UTC+8.
pub const TESTNET_L2_TIME_FORK_TIME: u64 = 1_764_842_400;

/// Wrong L2 time originally written into mainnet `rollup.json`.
pub const MAINNET_OLD_L2_TIME: u64 = 1_761_567_143;
/// Correct L2 time matching mainnet `genesis.json`.
pub const MAINNET_FIXED_L2_TIME: u64 = 1_761_579_057;
/// Wrong L2 time originally written into testnet `rollup.json`.
pub const TESTNET_OLD_L2_TIME: u64 = 1_760_699_568;
/// Correct L2 time matching testnet `genesis.json`.
pub const TESTNET_FIXED_L2_TIME: u64 = 1_760_700_537;

/// Returns the absolute start time for the first block of a span batch,
/// applying the X Layer L2 time fork when the chain ID matches.
///
/// For non-X Layer chains, returns `genesis_timestamp + rel_timestamp`
/// (the unmodified upstream behavior).
pub const fn get_batch_start_time(
    genesis_timestamp: u64,
    rel_timestamp: u64,
    chain_id: u64,
) -> u64 {
    let raw = genesis_timestamp + rel_timestamp;
    match chain_id {
        XLAYER_MAINNET_CHAIN_ID => {
            if raw < MAINNET_L2_TIME_FORK_TIME {
                MAINNET_OLD_L2_TIME + rel_timestamp
            } else {
                MAINNET_FIXED_L2_TIME + rel_timestamp
            }
        }
        XLAYER_TESTNET_CHAIN_ID => {
            if raw < TESTNET_L2_TIME_FORK_TIME {
                TESTNET_OLD_L2_TIME + rel_timestamp
            } else {
                TESTNET_FIXED_L2_TIME + rel_timestamp
            }
        }
        _ => raw,
    }
}

/// Auto-fixes the wrong `genesis.l2_time` shipped in X Layer mainnet/testnet
/// `rollup.json`. Mirrors `FixXLayerL2Time` from
/// `op-node/rollup/config_xlayer.go` (okx/optimism PR #178).
///
/// For non-X Layer chains this is a no-op. For X Layer chains, if the loaded
/// `l2_time` doesn't match the corrected value, it's overwritten in memory
/// and the file at `path` is rewritten so subsequent runs start with the
/// right value. File-write failure is logged but not fatal.
#[cfg(all(feature = "std", feature = "serde"))]
pub fn fix_xlayer_l2_time(cfg: &mut kona_genesis::RollupConfig, path: &std::path::Path) {
    let target = match cfg.l2_chain_id.id() {
        XLAYER_MAINNET_CHAIN_ID => MAINNET_FIXED_L2_TIME,
        XLAYER_TESTNET_CHAIN_ID => TESTNET_FIXED_L2_TIME,
        _ => return,
    };
    if cfg.genesis.l2_time == target {
        return;
    }
    tracing::warn!(
        target: "rollup_node",
        "X Layer: auto-fixing genesis.l2_time {} -> {}",
        cfg.genesis.l2_time, target
    );
    cfg.genesis.l2_time = target;

    match serde_json::to_string_pretty(cfg) {
        Ok(s) => match std::fs::write(path, s) {
            Ok(()) => tracing::info!(
                target: "rollup_node",
                "X Layer: saved fixed rollup config to {:?}",
                path
            ),
            Err(e) => tracing::warn!(
                target: "rollup_node",
                "X Layer: failed to persist fixed rollup config to {:?}: {e}",
                path
            ),
        },
        Err(e) => tracing::warn!(
            target: "rollup_node",
            "X Layer: failed to serialize fixed rollup config: {e}"
        ),
    }
}

#[cfg(all(test, feature = "std", feature = "serde"))]
mod tests {
    use super::*;
    use kona_genesis::RollupConfig;
    use std::fs::{self, File};
    use std::path::Path;

    /// Mirrors `TestFixXLayerL2Time` from
    /// `op-node/rollup/config_xlayer_test.go` (okx/optimism PR #178).
    ///
    /// The fixture lives at `op-node/rollup/rollup-unit-test.json` and is
    /// shared with the Go test. We copy it to a process-local temp path
    /// first so this test does not mutate the file in-place (which would
    /// race with the Go test and dirty the working tree).
    #[test]
    fn test_fix_xlayer_l2_time() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../../op-node/rollup/rollup-unit-test.json");
        let rollup_config_path = std::env::temp_dir()
            .join(format!("kona-rollup-unit-test-{}.json", std::process::id()));
        fs::copy(&src, &rollup_config_path).expect("copy fixture");

        // 1. Read ./rollup-unit-test.json
        let mut cfg = read_config(&rollup_config_path).expect("read fixture");

        // 2. Verify L2Time was not fixed
        let init_l2_time = cfg.genesis.l2_time;
        assert_eq!(init_l2_time, MAINNET_OLD_L2_TIME);

        // 3. Fix it by calling fix_xlayer_l2_time
        fix_xlayer_l2_time(&mut cfg, &rollup_config_path);

        // 4. Verify L2Time was fixed
        assert_eq!(
            cfg.genesis.l2_time, MAINNET_FIXED_L2_TIME,
            "L2Time should be fixed to MainnetFixedL2Time"
        );

        // 5. Verify the file was updated
        let saved_cfg = read_config(&rollup_config_path).expect("re-read fixture");
        assert_eq!(
            saved_cfg.genesis.l2_time, MAINNET_FIXED_L2_TIME,
            "L2Time should be fixed to MainnetFixedL2Time"
        );

        // 6. revert L2 time
        cfg.genesis.l2_time = init_l2_time;
        let s = serde_json::to_string_pretty(&cfg).expect("serialize");
        fs::write(&rollup_config_path, s).expect("write fixture back");
    }

    fn read_config(path: &Path) -> Result<RollupConfig, Box<dyn std::error::Error>> {
        let file = File::open(path)?;
        Ok(serde_json::from_reader(file)?)
    }
}
