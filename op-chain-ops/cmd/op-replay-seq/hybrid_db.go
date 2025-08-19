package main

import (
	"fmt"
	"sync"

	"github.com/ethereum/go-ethereum/core/rawdb"
	"github.com/ethereum/go-ethereum/ethdb"
	"github.com/ethereum/go-ethereum/ethdb/memorydb"
	"github.com/ethereum/go-ethereum/ethdb/remotedb"
	"github.com/ethereum/go-ethereum/rpc"
)

// =============================================================================
// Hybrid Database Implementation
// =============================================================================

// hybridRemoteDB combines remotedb (read-only) with memdb (write cache)
// This solves the NewBatch issue by writing to memdb while reading from both sources
type hybridRemoteDB struct {
	ethdb.Database                // Embed the interface to inherit all methods
	memDB          ethdb.Database // Write cache for all modifications
	mu             sync.RWMutex   // Protect concurrent access
}

// NewHybridRemoteDB creates a new hybrid database that combines remotedb and memdb
func NewHybridRemoteDB(cl *rpc.Client) *hybridRemoteDB {
	remoteDB := remotedb.New(cl)
	return &hybridRemoteDB{
		Database: remoteDB, // Embed the remote database
		memDB:    rawdb.NewDatabase(memorydb.New()),
	}
}

// NewBatch returns a batch that writes to memdb
func (h *hybridRemoteDB) NewBatch() ethdb.Batch {
	return h.memDB.NewBatch()
}

// Get implements layered read strategy: memdb first, then remotedb
func (h *hybridRemoteDB) Get(key []byte) ([]byte, error) {
	h.mu.RLock()
	defer h.mu.RUnlock()

	// First try memdb (most recent data)
	if val, err := h.memDB.Get(key); err == nil {
		return val, nil
	}

	// If not in memdb, try remotedb (initial state)
	return h.Database.Get(key)
}

// Has implements layered read strategy
func (h *hybridRemoteDB) Has(key []byte) (bool, error) {
	h.mu.RLock()
	defer h.mu.RUnlock()

	// First check memdb
	if has, err := h.memDB.Has(key); err == nil && has {
		return true, nil
	}

	// Then check remotedb
	return h.Database.Has(key)
}

// Put writes to memdb only
func (h *hybridRemoteDB) Put(key, value []byte) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.memDB.Put(key, value)
}

// Delete removes from memdb only
func (h *hybridRemoteDB) Delete(key []byte) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.memDB.Delete(key)
}

// Close closes both databases
func (h *hybridRemoteDB) Close() error {
	h.mu.Lock()
	defer h.mu.Unlock()

	// Close memdb first
	if err := h.memDB.Close(); err != nil {
		return err
	}

	// Then close remotedb
	return h.Database.Close()
}

// For all other methods, delegate to remoteDB (read-only operations)
func (h *hybridRemoteDB) Stat() (string, error) { return h.Database.Stat() }

func (h *hybridRemoteDB) Compact(start []byte, limit []byte) error {
	return h.Database.Compact(start, limit)
}

func (h *hybridRemoteDB) NewIterator(prefix []byte, start []byte) ethdb.Iterator {
	return h.Database.NewIterator(prefix, start)
}

func (h *hybridRemoteDB) Ancient(kind string, number uint64) ([]byte, error) {
	return h.Database.Ancient(kind, number)
}

func (h *hybridRemoteDB) AncientRange(kind string, start, count, maxBytes uint64) ([][]byte, error) {
	return h.Database.AncientRange(kind, start, count, maxBytes)
}

func (h *hybridRemoteDB) Ancients() (uint64, error) { return h.Database.Ancients() }

func (h *hybridRemoteDB) Tail() (uint64, error) { return h.Database.Tail() }

func (h *hybridRemoteDB) AncientSize(kind string) (uint64, error) {
	return h.Database.AncientSize(kind)
}

func (h *hybridRemoteDB) ReadAncients(fn func(reader ethdb.AncientReaderOp) error) (err error) {
	// 使用类型断言来调用正确的方法
	if reader, ok := h.Database.(interface {
		ReadAncients(func(ethdb.AncientReaderOp) error) error
	}); ok {
		return reader.ReadAncients(fn)
	}
	// 如果底层数据库不支持，返回错误
	return fmt.Errorf("underlying database does not support ReadAncients")
}

func (h *hybridRemoteDB) Sync() error { return h.Database.Sync() }

func (h *hybridRemoteDB) AncientDatadir() (string, error) { return h.Database.AncientDatadir() }

func (h *hybridRemoteDB) DeleteRange(start []byte, limit []byte) error {
	return h.Database.DeleteRange(start, limit)
}

func (h *hybridRemoteDB) HasAncient(kind string, number uint64) (bool, error) {
	return h.Database.HasAncient(kind, number)
}

func (h *hybridRemoteDB) ModifyAncients(fn func(ethdb.AncientWriteOp) error) (int64, error) {
	// 检查底层数据库是否支持 ModifyAncients
	if modifier, ok := h.Database.(interface {
		ModifyAncients(func(ethdb.AncientWriteOp) error) (int64, error)
	}); ok {
		return modifier.ModifyAncients(fn)
	}
	// 如果不支持，返回默认值
	return 0, fmt.Errorf("underlying database does not support ModifyAncients")
}
