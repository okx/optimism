# op-proposer/mock

Test utilities for op-proposer, including a mock TeeRollup HTTP server.

---

## Mock TeeRollup Server

Simulates the `GET /v1/chain/confirmed_block_info` REST endpoint provided by a real TeeRollup service.

**Behavior:**
- Starts at block height 1000 (configurable)
- Increments height by a random delta in **[1, 50]** every second
- `appHash` = `keccak256(big-endian uint64 of height)`, `"0x"` prefix, 66 characters
- `blockHash` = `keccak256(appHash)`, `"0x"` prefix, 66 characters

---

## How to Run

### Option 1: Direct `go run` (recommended, no build step)

```bash
# From the op-proposer directory
cd op-proposer
go run ./mock/cmd/mockteerpc

# Custom listen address and initial height
go run ./mock/cmd/mockteerpc --addr :9000 --init-height 5000

# 30% error rate + max 500ms delay
go run ./mock/cmd/mockteerpc --error-rate 0.3 --delay 500ms
```

### Option 2: Build then run

```bash
cd op-proposer
go build -o bin/mockteerpc ./mock/cmd/mockteerpc
./bin/mockteerpc --addr :8090
```

Startup output example:
```
mock TeeRollup server listening on :8090
initial height: 1000
endpoint: GET /v1/chain/confirmed_block_info

tick: height=1023 delta=23
tick: height=1058 delta=35
...
```

---

## curl Testing

```bash
# Query current confirmed block info
curl -s http://localhost:8090/v1/chain/confirmed_block_info | jq .
```

Example response:
```json
{
  "code": 0,
  "message": "OK",
  "data": {
    "height": 1023,
    "appHash": "0x3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b",
    "blockHash": "0x1234abcd..."
  }
}
```

### Observe height growth continuously

```bash
# Request every 0.5s to observe height changes
watch -n 0.5 'curl -s http://localhost:8090/v1/chain/confirmed_block_info | jq .data'
```

### Verify hash computation (Python)

```python
from eth_hash.auto import keccak
import struct, requests

r = requests.get("http://localhost:8090/v1/chain/confirmed_block_info").json()
height = r["data"]["height"]

app_hash = keccak(struct.pack(">Q", height)).hex()
block_hash = keccak(bytes.fromhex(app_hash)).hex()

print(f"height:    {height}")
print(f"appHash:   0x{app_hash}")   # 66 chars
print(f"blockHash: 0x{block_hash}") # 66 chars
# Should match API response
```

---

## Usage in Tests

```go
import "github.com/ethereum-optimism/optimism/op-proposer/mock"

func TestMyFeature(t *testing.T) {
    srv := mock.NewTeeRollupServer(t)  // t.Cleanup closes automatically

    // Server URL
    baseURL := srv.Addr()  // e.g. "http://127.0.0.1:12345"

    // Get current snapshot (no HTTP request needed)
    height, appHash, blockHash := srv.CurrentInfo()
    _ = height
    _ = appHash
    _ = blockHash
}
```

---

## CLI flags

| Flag           | Default | Description                                                              |
|----------------|---------|--------------------------------------------------------------------------|
| `--addr`       | `:8090` | Listen address                                                           |
| `--init-height`| `1000`  | Initial block height                                                     |
| `--error-rate` | `0`     | Error response probability [0.0, 1.0], 0 means no errors                |
| `--delay`      | `1s`    | Maximum random response delay, actual delay is random in [0, delay] (supports `500ms`, `2s`) |
