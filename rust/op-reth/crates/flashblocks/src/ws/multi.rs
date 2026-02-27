use crate::FlashBlock;
use alloy_primitives::B256;
use alloy_rpc_types_engine::PayloadId;
use futures_util::Stream;
use metrics::{Counter, Gauge};
use reth_metrics::Metrics;
use std::{
    collections::{hash_map::Entry, HashMap},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Sleep;
use tracing::warn;

/// Backoff duration applied per-source when a stream error occurs.
const PER_SOURCE_BACKOFF: Duration = Duration::from_millis(500);

/// Dedup key: uniquely identifies a flashblock within a block.
type DedupKey = (PayloadId, u64);

/// Per-source state tracker.
#[derive(Debug)]
struct SourceState {
    index: usize,
    terminated: bool,
    backoff: Option<Pin<Box<Sleep>>>,
}

/// A multi-source flashblock stream that wraps multiple inner streams.
///
/// Deduplicates flashblocks by `(payload_id, index)`. The first-arriving
/// flashblock for each key wins. Subsequent duplicates are cross-validated
/// by comparing block hashes, with mismatches logged as warnings.
///
/// Each source has independent error backoff. When a source errors, it backs
/// off for [`PER_SOURCE_BACKOFF`] before being polled again. Other sources
/// continue serving flashblocks during this time.
#[derive(Debug)]
pub struct MultiSourceFlashBlockStream<S> {
    sources: Vec<(S, SourceState)>,
    seen: HashMap<DedupKey, B256>,
    current_payload_id: Option<PayloadId>,
    poll_offset: usize,
    metrics: MultiSourceMetrics,
}

impl<S> MultiSourceFlashBlockStream<S> {
    /// Creates a new multi-source stream wrapping the given inner streams.
    pub fn new(streams: Vec<S>) -> Self {
        let len = streams.len();
        let sources = streams
            .into_iter()
            .enumerate()
            .map(|(i, s)| (s, SourceState { index: i, terminated: false, backoff: None }))
            .collect();
        let metrics = MultiSourceMetrics::default();
        metrics.total_sources.set(len as f64);
        metrics.active_sources.set(len as f64);
        Self { sources, seen: HashMap::new(), current_payload_id: None, poll_offset: 0, metrics }
    }

    /// Updates the active sources gauge metric.
    fn update_active_sources_metric(&self) {
        let active = self.sources.iter().filter(|(_, s)| !s.terminated).count();
        self.metrics.active_sources.set(active as f64);
    }
}

impl<S> Stream for MultiSourceFlashBlockStream<S>
where
    S: Stream<Item = eyre::Result<FlashBlock>> + Unpin,
{
    type Item = eyre::Result<FlashBlock>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let num_sources = this.sources.len();

        if num_sources == 0 {
            return Poll::Ready(None);
        }

        // Rotate start index
        let start = this.poll_offset;
        this.poll_offset = (this.poll_offset + 1) % num_sources;

        for i in 0..num_sources {
            let idx = (start + i) % num_sources;
            let (stream, state) = &mut this.sources[idx];

            if state.terminated {
                continue;
            }

            if let Some(backoff) = &mut state.backoff {
                if backoff.as_mut().poll(cx).is_pending() {
                    continue;
                }
                state.backoff = None;
            }

            match Pin::new(stream).poll_next(cx) {
                Poll::Ready(Some(Ok(flashblock))) => {
                    // Detect new block: index 0 with a new payload_id resets dedup state
                    if flashblock.index == 0 &&
                        this.current_payload_id != Some(flashblock.payload_id)
                    {
                        this.seen.clear();
                        this.current_payload_id = Some(flashblock.payload_id);
                    }

                    let key = (flashblock.payload_id, flashblock.index);

                    match this.seen.entry(key) {
                        Entry::Vacant(entry) => {
                            // First arrival
                            entry.insert(flashblock.diff.block_hash);
                            return Poll::Ready(Some(Ok(flashblock)));
                        }
                        Entry::Occupied(entry) => {
                            // Duplicate - cross-validate and skip
                            this.metrics.deduplicated_total.increment(1);
                            let existing_hash = *entry.get();
                            if existing_hash != flashblock.diff.block_hash {
                                warn!(
                                    target: "flashblocks",
                                    source = state.index,
                                    payload_id = %flashblock.payload_id,
                                    index = flashblock.index,
                                    expected_hash = %existing_hash,
                                    actual_hash = %flashblock.diff.block_hash,
                                    "Cross-validation mismatch: block hash differs between sources"
                                );
                                this.metrics.mismatch_total.increment(1);
                            }
                        }
                    }
                }
                Poll::Ready(Some(Err(err))) => {
                    warn!(
                        target: "flashblocks",
                        source = state.index,
                        %err,
                        "Source error, backing off"
                    );
                    let mut backoff = Box::pin(tokio::time::sleep(PER_SOURCE_BACKOFF));
                    // Register waker so we're notified when the backoff expires
                    let _ = backoff.as_mut().poll(cx);
                    state.backoff = Some(backoff);
                }
                Poll::Ready(None) => {
                    state.terminated = true;
                    warn!(target: "flashblocks", source = state.index, "Source terminated");
                    this.update_active_sources_metric();
                }
                Poll::Pending => {}
            }
        }

        if this.sources.iter().all(|(_, s)| s.terminated) {
            warn!(target: "flashblocks", "All sources terminated, returning None");
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

/// Metrics for [`MultiSourceFlashBlockStream`].
#[derive(Metrics)]
#[metrics(scope = "flashblock_multi_source")]
struct MultiSourceMetrics {
    /// Total flashblocks dropped as duplicates.
    deduplicated_total: Counter,
    /// Cross-validation hash mismatches detected.
    mismatch_total: Counter,
    /// Currently active (non-terminated) sources.
    active_sources: Gauge,
    /// Total configured sources.
    total_sources: Gauge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestFlashBlockFactory;
    use futures_util::{stream, StreamExt};

    /// A concrete stream type used in tests to allow mixing ok/error items in the same `Vec`.
    type TestStream = stream::Iter<std::vec::IntoIter<eyre::Result<FlashBlock>>>;

    fn ok_stream(items: Vec<FlashBlock>) -> TestStream {
        stream::iter(items.into_iter().map(Ok).collect::<Vec<_>>())
    }

    fn err_then_done(msg: &str) -> TestStream {
        stream::iter(vec![Err(eyre::eyre!("{msg}"))])
    }

    fn mixed_stream(items: Vec<eyre::Result<FlashBlock>>) -> TestStream {
        stream::iter(items)
    }

    #[tokio::test]
    async fn test_single_source_passthrough() {
        let factory = TestFlashBlockFactory::new();
        let fb0 = factory.flashblock_at(0).build();
        let fb1 = factory.flashblock_after(&fb0).build();
        let fb2 = factory.flashblock_after(&fb1).build();

        let mut stream = MultiSourceFlashBlockStream::new(vec![ok_stream(vec![
            fb0.clone(),
            fb1.clone(),
            fb2.clone(),
        ])]);

        let r0 = stream.next().await.unwrap().unwrap();
        let r1 = stream.next().await.unwrap().unwrap();
        let r2 = stream.next().await.unwrap().unwrap();

        assert_eq!(r0, fb0);
        assert_eq!(r1, fb1);
        assert_eq!(r2, fb2);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_first_arrival_dedup() {
        let factory = TestFlashBlockFactory::new();
        let fb0 = factory.flashblock_at(0).build();
        let fb1 = factory.flashblock_after(&fb0).build();

        // Both sources have the same flashblocks.
        let source_a = ok_stream(vec![fb0.clone(), fb1.clone()]);
        let source_b = ok_stream(vec![fb0.clone(), fb1.clone()]);

        let stream = MultiSourceFlashBlockStream::new(vec![source_a, source_b]);
        let results: Vec<_> = stream.map(Result::unwrap).collect().await;

        // Only 2 unique flashblocks emitted, not 4.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], fb0);
        assert_eq!(results[1], fb1);
    }

    #[tokio::test]
    async fn test_cross_validation_mismatch() {
        let factory = TestFlashBlockFactory::new();
        let fb_a = factory.flashblock_at(0).block_hash(B256::with_last_byte(1)).build();
        let fb_b = factory
            .flashblock_at(0)
            .payload_id(fb_a.payload_id)
            .block_hash(B256::with_last_byte(2))
            .build();

        let source_a = ok_stream(vec![fb_a.clone()]);
        let source_b = ok_stream(vec![fb_b]);

        let mut stream = MultiSourceFlashBlockStream::new(vec![source_a, source_b]);

        // First arrival wins.
        let result = stream.next().await.unwrap().unwrap();
        assert_eq!(result.diff.block_hash, fb_a.diff.block_hash);

        // Duplicate with mismatched hash is dropped (mismatch metric incremented).
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_per_source_error_backoff() {
        let factory = TestFlashBlockFactory::new();
        let fb0 = factory.flashblock_at(0).build();

        // Source A errors, source B delivers fb0.
        let source_a = err_then_done("connection lost");
        let source_b = ok_stream(vec![fb0.clone()]);

        let mut stream = MultiSourceFlashBlockStream::new(vec![source_a, source_b]);

        // Source B's flashblock should come through despite source A's error.
        let result = stream.next().await.unwrap().unwrap();
        assert_eq!(result, fb0);
    }

    #[tokio::test]
    async fn test_error_mixed_with_ok() {
        let factory = TestFlashBlockFactory::new();
        let fb0 = factory.flashblock_at(0).build();
        let fb1 = factory.flashblock_after(&fb0).build();

        // Source A: error then fb1. Source B: fb0.
        let source_a = mixed_stream(vec![Err(eyre::eyre!("oops")), Ok(fb1.clone())]);
        let source_b = ok_stream(vec![fb0.clone()]);

        let mut stream = MultiSourceFlashBlockStream::new(vec![source_a, source_b]);

        // fb0 from source B arrives (source A is backing off).
        let result = stream.next().await.unwrap().unwrap();
        assert_eq!(result, fb0);
    }

    #[tokio::test]
    async fn test_source_termination_liveness() {
        let factory = TestFlashBlockFactory::new();
        let fb0 = factory.flashblock_at(0).build();
        let fb1 = factory.flashblock_after(&fb0).build();

        // Source A has fb0 then ends, source B has fb0 (dup) then fb1.
        let source_a = ok_stream(vec![fb0.clone()]);
        let source_b = ok_stream(vec![fb0.clone(), fb1.clone()]);

        let stream = MultiSourceFlashBlockStream::new(vec![source_a, source_b]);
        let results: Vec<_> = stream.map(Result::unwrap).collect().await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0], fb0);
        assert_eq!(results[1], fb1);
    }

    #[tokio::test]
    async fn test_new_block_resets_dedup() {
        let factory = TestFlashBlockFactory::new();

        // Block 1
        let b1_fb0 = factory.flashblock_at(0).build();
        // Block 2 (new payload_id, index 0)
        let b2_fb0 = factory.flashblock_for_next_block(&b1_fb0).build();

        // Source A: block1 fb0, block2 fb0
        // Source B: block1 fb0 (dup), block2 fb0 (dup)
        let source_a = ok_stream(vec![b1_fb0.clone(), b2_fb0.clone()]);
        let source_b = ok_stream(vec![b1_fb0.clone(), b2_fb0.clone()]);

        let stream = MultiSourceFlashBlockStream::new(vec![source_a, source_b]);
        let results: Vec<_> = stream.map(Result::unwrap).collect().await;

        // 2 unique flashblocks: one per block.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].payload_id, b1_fb0.payload_id);
        assert_eq!(results[1].payload_id, b2_fb0.payload_id);
    }

    #[tokio::test]
    async fn test_all_done_returns_none() {
        let source_a: TestStream = stream::iter(vec![]);
        let source_b: TestStream = stream::iter(vec![]);

        let mut stream = MultiSourceFlashBlockStream::new(vec![source_a, source_b]);
        assert!(stream.next().await.is_none());
    }
}
