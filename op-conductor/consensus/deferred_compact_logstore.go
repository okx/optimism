// DeferredCompactLogStore: see op-conductor/docs/snapshot-compact.md for design
// and snapshot/compact tuning (e.g. SnapshotThreshold to reduce per-compact amount).
package consensus

import (
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/log"
	"github.com/hashicorp/raft"
)

const (
	// defaultCompactBatchSize is the number of log entries to delete per batch
	// during deferred compaction. Smaller batches release the LogStore lock more
	// often so StoreLogs (Apply path) can proceed; aim for each batch < ~200ms.
	defaultCompactBatchSize = 200
	// defaultCompactBatchPause is a short yield between batches so the main
	// loop can acquire the store lock for StoreLogs.
	defaultCompactBatchPause = 5 * time.Millisecond
)

// DeferredCompactLogStore wraps a raft.LogStore and runs DeleteRange in a
// background goroutine in small batches. This avoids blocking the Raft main
// loop (and thus Apply/StoreLogs) for the full duration of compaction when
// snapshot triggers compactLogs; each batch holds the store lock briefly so
// CommitUnsafePayload can complete within block time (e.g. 1s).
type DeferredCompactLogStore struct {
	log       log.Logger
	store     raft.LogStore
	batchSize uint64
	pause     time.Duration
	mu        sync.Mutex
}

// NewDeferredCompactLogStore returns a LogStore that defers DeleteRange to
// background batched deletes. batchSize and pause can be 0 to use defaults.
func NewDeferredCompactLogStore(log log.Logger, store raft.LogStore, batchSize uint64, pause time.Duration) *DeferredCompactLogStore {
	if batchSize == 0 {
		batchSize = defaultCompactBatchSize
	}
	if pause == 0 {
		pause = defaultCompactBatchPause
	}
	return &DeferredCompactLogStore{
		log:       log,
		store:     store,
		batchSize: batchSize,
		pause:     pause,
	}
}

// FirstIndex implements raft.LogStore.
func (d *DeferredCompactLogStore) FirstIndex() (uint64, error) {
	return d.store.FirstIndex()
}

// LastIndex implements raft.LogStore.
func (d *DeferredCompactLogStore) LastIndex() (uint64, error) {
	return d.store.LastIndex()
}

// GetLog implements raft.LogStore.
func (d *DeferredCompactLogStore) GetLog(index uint64, log *raft.Log) error {
	return d.store.GetLog(index, log)
}

// StoreLog implements raft.LogStore.
func (d *DeferredCompactLogStore) StoreLog(log *raft.Log) error {
	return d.store.StoreLog(log)
}

// StoreLogs implements raft.LogStore.
func (d *DeferredCompactLogStore) StoreLogs(logs []*raft.Log) error {
	return d.store.StoreLogs(logs)
}

// DeleteRange implements raft.LogStore. It returns immediately and performs
// the actual delete in a background goroutine in batches, so the Raft main
// loop is not blocked for the full compaction duration.
func (d *DeferredCompactLogStore) DeleteRange(min, max uint64) error {
	go d.runDeleteRange(min, max)
	return nil
}

func (d *DeferredCompactLogStore) runDeleteRange(min, max uint64) {
	d.mu.Lock()
	defer d.mu.Unlock()

	for min <= max {
		batchEnd := min + d.batchSize - 1
		if batchEnd > max {
			batchEnd = max
		}
		if err := d.store.DeleteRange(min, batchEnd); err != nil {
			d.log.Error("deferred compact batch failed", "min", min, "max", batchEnd, "err", err)
			return
		}
		min = batchEnd + 1
		if min <= max {
			time.Sleep(d.pause)
		}
	}
	d.log.Debug("deferred compact completed", "up_to", max)
}
