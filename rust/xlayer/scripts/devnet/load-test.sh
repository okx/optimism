#!/bin/bash
# load-test.sh — xlayer-node stress test + Engine API latency benchmark
#
# Drives concurrent native-ETH transfers from multiple genesis-funded senders
# while simultaneously probing the Engine API (FCU / getPayload) for latency.
# All results are reported in a single structured report, making this script
# suitable for direct comparison between xlayer-node (in-process) and a
# standard reth + op-node (HTTP) setup.
#
# TX robustness guarantees
#   - SIGINT / SIGTERM → workers stop cleanly → partial report still produced
#   - _timeout() wraps cast send; a hung RPC never stalls a worker forever
#   - nonce-too-low, already-known, mempool-full, RPC-error all handled distinctly
#   - periodic nonce resync every NONCE_RESYNC_EVERY successful sends
#   - receipt polling sweeps ALL pending TXs each round (POLL_BATCH concurrent)
#   - block stat and receipt fetches use bash-3.2-compatible arrays
#
# Engine API probe (FCU / getPayload)
#   - Baseline: N FCU pings + M getPayload cycles before load starts
#   - Under-load: FCU sampled every ~5 s during the load phase
#   - Post-load: N FCU pings after workers stop
#   - Reports min/avg/p50/p95/p99/max per call type
#   - Portable: works for any setup that exposes an authrpc endpoint
#
# Prerequisites: cast (Foundry), jq, curl, python3
#
# Usage:
#   ./scripts/devnet/load-test.sh                           # 60s, 6 workers
#   ./scripts/devnet/load-test.sh --duration 120            # longer run
#   ./scripts/devnet/load-test.sh --concurrency 4           # fewer workers
#   ./scripts/devnet/load-test.sh --no-wait-safe            # skip safe-head wait
#   ./scripts/devnet/load-test.sh --no-engine-probe         # skip Engine API probe
#   ./scripts/devnet/load-test.sh --out results/run1.txt    # custom report file
#
# Environment overrides (can also be set in .env):
#   AUTH_RPC_URL       Engine API authrpc (default: http://127.0.0.1:8552)
#   JWT_SECRET_PATH    Path to JWT secret hex file (default: config/devnet/jwt.txt)

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

# ── Tunables ───────────────────────────────────────────────────────────────────
DURATION=60          # seconds of active load
CONCURRENCY=6        # sender workers  (max = len(SENDER_KEYS))
GAS_PRICE_GWEI=110   # static gas price; well above devnet base fee
CAST_TIMEOUT=8       # hard timeout (s) per cast send — prevents RPC hangs
CONFIRM_TIMEOUT=120  # seconds to poll for unsafe inclusion after load stops
SAFE_WAIT=180        # seconds to wait for safe head after confirmation
WAIT_SAFE=true
NONCE_RESYNC_EVERY=100   # resync nonce from chain every N successful sends
POLL_BATCH=100           # concurrent receipt checks per sub-batch

# Engine API probe config
AUTH_RPC_URL="${AUTH_RPC_URL:-http://127.0.0.1:8552}"
JWT_SECRET_PATH="${JWT_SECRET_PATH:-config/devnet/jwt.txt}"
ENGINE_PROBE_N=20    # FCU pings per probe window (baseline + post-load)
ENGINE_GP_N=5        # getPayload cycles in baseline probe
MEASURE_ENGINE=true  # disabled by --no-engine-probe

OUT_FILE=""

# ── Genesis-funded senders ─────────────────────────────────────────────────────
# Standard Hardhat accounts (mnemonic: "test test … test junk"), all allocated
# 10 000 ETH in xlayer-node devnet genesis — verified against genesis.json alloc.
SENDER_KEYS=(
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"  # 0x70997970
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"  # 0x3C44CdDd
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"  # 0x90F79bf6
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926b"  # 0x15d34AAf
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"  # 0x9965507D
    "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e"  # 0x976EA740
    "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356"  # 0x14dC7996
)
RECIPIENT="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"   # Hardhat #0
EXPECTED_CHAIN_ID=195

# ── Arg parsing ────────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --duration)        DURATION="$2";    shift 2 ;;
        --concurrency)     CONCURRENCY="$2"; shift 2 ;;
        --out)             OUT_FILE="$2";    shift 2 ;;
        --no-wait-safe)    WAIT_SAFE=false;  shift   ;;
        --no-engine-probe) MEASURE_ENGINE=false; shift ;;
        *) warn "Unknown flag: $1 (ignored)"; shift ;;
    esac
done

MAX_WORKERS=${#SENDER_KEYS[@]}
[[ $CONCURRENCY -gt $MAX_WORKERS ]] && CONCURRENCY=$MAX_WORKERS
[[ -z "$OUT_FILE" ]] && OUT_FILE="load-test-$(date +%Y%m%d_%H%M%S).txt"

# ── Helpers ────────────────────────────────────────────────────────────────────
ms_now()  { python3 -c "import time; print(int(time.time()*1000))"; }

# to_int_py: inline python snippet — handles both "0x1a2b" and "6699" (decimal)
to_int_py='
def to_int(v, default=0):
    if v is None: return default
    if isinstance(v, int): return v
    s = str(v).strip()
    if s.startswith("0x") or s.startswith("0X"):
        try: return int(s, 16)
        except: return default
    try: return int(s)
    except:
        try: return int(s, 16)
        except: return default
'

# hex2dec: accepts "0x1a2b" (hex) or "6699" (decimal int string)
hex2dec() {
    local v="${1:-0}"
    python3 -c "
${to_int_py}
print(to_int('${v}'))
" 2>/dev/null || echo 0
}

# _timeout — GNU timeout / gtimeout fallback / bare exec (macOS compat)
_timeout() {
    local secs="$1"; shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$secs" "$@"
    elif command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$secs" "$@"
    else
        "$@"
    fi
}

sync_status() {
    curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
        2>/dev/null || echo "{}"
}

l2_heads() {
    # Returns: unsafe_num safe_num fin_num  (decimal, space-separated)
    local s; s=$(sync_status)
    local u s2 f
    u=$(echo "$s"  | jq -r '.result.unsafe_l2.number    // "0x0"')
    s2=$(echo "$s" | jq -r '.result.safe_l2.number      // "0x0"')
    f=$(echo "$s"  | jq -r '.result.finalized_l2.number // "0x0"')
    echo "$(hex2dec "$u") $(hex2dec "$s2") $(hex2dec "$f")"
}

# ── Engine API probe ───────────────────────────────────────────────────────────
# run_engine_probe OUT_FILE LABEL
#   Runs ENGINE_PROBE_N FCU pings and ENGINE_GP_N getPayload cycles.
#   Appends one line per call to OUT_FILE: "type<TAB>latency_ms"
#   Works for any authrpc endpoint (xlayer in-process OR reth+op-node HTTP).
run_engine_probe() {
    local out_file="$1"
    local label="${2:-probe}"

    if [[ "$MEASURE_ENGINE" != "true" ]]; then return; fi
    if [[ ! -f "$JWT_SECRET_PATH" ]]; then
        warn "JWT secret not found at $JWT_SECRET_PATH — engine probe skipped"
        MEASURE_ENGINE=false
        return
    fi

    python3 - "$out_file" "$AUTH_RPC_URL" "$JWT_SECRET_PATH" \
              "$L2_RPC_URL" "$ENGINE_PROBE_N" "$ENGINE_GP_N" "$label" \
<<'PROBE_PY'
import sys, json, hmac, hashlib, base64, time, urllib.request

out_file, engine_url, jwt_path, l2_url = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
n_fcu, n_gp, label = int(sys.argv[5]), int(sys.argv[6]), sys.argv[7]

def gen_jwt(secret_hex):
    secret = bytes.fromhex(secret_hex.strip())
    hdr = base64.urlsafe_b64encode(b'{"typ":"JWT","alg":"HS256"}').rstrip(b'=').decode()
    pay = base64.urlsafe_b64encode(json.dumps({"iat": int(time.time())}).encode()).rstrip(b'=').decode()
    msg = f'{hdr}.{pay}'.encode()
    sig = hmac.new(secret, msg, hashlib.sha256).digest()
    return f'{hdr}.{pay}.{base64.urlsafe_b64encode(sig).rstrip(b"=").decode()}'

def engine_call(body, jwt):
    req = urllib.request.Request(
        engine_url,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {jwt}"},
        method="POST")
    t0 = time.perf_counter()
    with urllib.request.urlopen(req, timeout=5) as r:
        resp = json.loads(r.read())
    lat_ms = (time.perf_counter() - t0) * 1000
    return resp, lat_ms

def eth_call(method, params):
    req = urllib.request.Request(
        l2_url,
        data=json.dumps({"jsonrpc":"2.0","id":1,"method":method,"params":params}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST")
    with urllib.request.urlopen(req, timeout=5) as r:
        return json.loads(r.read()).get("result")

try:
    secret_hex = open(jwt_path).read().strip()
except Exception as e:
    print(f"Cannot read JWT: {e}", file=sys.stderr)
    sys.exit(0)  # soft-fail — don't abort the whole load test

# ── Fetch chain state ──────────────────────────────────────────────────────────
try:
    sync = eth_call("optimism_syncStatus", [])
    unsafe = sync.get("unsafe_l2", {}) if sync else {}
    safe   = sync.get("safe_l2",   {}) if sync else {}
    fin    = sync.get("finalized_l2", {}) if sync else {}

    head_hash = unsafe.get("hash", "0x" + "0"*64)
    safe_hash = safe.get("hash",   head_hash)
    fin_hash  = fin.get("hash",    head_hash)

    blk = eth_call("eth_getBlockByNumber", ["latest", False]) or {}
    def to_int(v):
        if isinstance(v, int): return v
        s = str(v).strip()
        return int(s, 16) if s.startswith("0x") else int(s)
    base_ts         = to_int(blk.get("timestamp", "0x0"))
    parent_beacon   = blk.get("parentBeaconBlockRoot", "0x" + "0"*64)
except Exception as e:
    print(f"Failed to fetch chain state: {e}", file=sys.stderr)
    sys.exit(0)

results = []

# ── Phase 1: FCU pings — no payloadAttributes (safe, just sets head) ───────────
fcu_no_attr = {
    "jsonrpc": "2.0", "id": 1,
    "method": "engine_forkchoiceUpdatedV3",
    "params": [
        {"headBlockHash": head_hash, "safeBlockHash": safe_hash, "finalizedBlockHash": fin_hash},
        None
    ]
}
for _ in range(n_fcu):
    try:
        _, lat = engine_call(fcu_no_attr, gen_jwt(secret_hex))
        results.append(f"{label}:fcu\t{lat:.3f}")
    except Exception:
        results.append(f"{label}:fcu_err\t0")

# ── Phase 2: FCU with payloadAttributes → getPayload ──────────────────────────
# Uses incrementing timestamp so each cycle produces a distinct payloadId.
# The payload is built but never submitted — purely for latency measurement.
for i in range(n_gp):
    ts = base_ts + 2 + i
    fcu_attrs = {
        "jsonrpc": "2.0", "id": 1,
        "method": "engine_forkchoiceUpdatedV3",
        "params": [
            {"headBlockHash": head_hash, "safeBlockHash": safe_hash, "finalizedBlockHash": fin_hash},
            {
                "timestamp":             hex(ts),
                "prevRandao":            "0x" + "0"*64,
                "suggestedFeeRecipient": "0x4200000000000000000000000000000000000011",
                "withdrawals":           [],
                "parentBeaconBlockRoot": parent_beacon
            }
        ]
    }
    try:
        resp, fcu_lat = engine_call(fcu_attrs, gen_jwt(secret_hex))
        results.append(f"{label}:fcu_attrs\t{fcu_lat:.3f}")

        payload_id = (resp.get("result") or {}).get("payloadId")
        if not payload_id:
            continue

        # Let payload builder work briefly before fetching
        time.sleep(0.15)

        gp_body = {"jsonrpc":"2.0","id":1,"method":"engine_getPayloadV4","params":[payload_id]}
        _, gp_lat = engine_call(gp_body, gen_jwt(secret_hex))
        results.append(f"{label}:getpayload\t{gp_lat:.3f}")
    except Exception:
        results.append(f"{label}:getpayload_err\t0")

with open(out_file, 'a') as f:
    for r in results:
        f.write(r + "\n")
PROBE_PY
}

# ── Sender worker ──────────────────────────────────────────────────────────────
# Runs in a bash subshell.  Owns one private key → one nonce sequence.
# Writes  "hash<TAB>send_ms"  to $out_file on every successful submit.
# Writes  "sent<SPACE>errs"   to $stats_file on clean exit.
#
# Error handling matrix:
#   nonce too low          → resync nonce from chain  (chain advanced past us)
#   already known          → advance nonce            (TX already queued)
#   replacement underpriced→ advance nonce            (same nonce in pool)
#   mempool / pool full    → resync + 500ms backoff   (node back-pressure)
#   connection / RPC error → 2s backoff               (transient connectivity)
#   5 consecutive unknown  → resync + 1s backoff      (catch-all recovery)
_lt_worker() {
    set +e   # worker handles its own errors; must not exit on non-zero
    local worker_id="$1" key="$2" out_file="$3" err_file="$4" \
          stats_file="$5" stop_file="$6"

    local addr
    addr=$(cast wallet address "$key" 2>/dev/null) || { echo "0 0" > "$stats_file"; return; }

    local nonce
    nonce=$(cast nonce --rpc-url "$L2_RPC_URL" "$addr" 2>/dev/null || echo 0)

    local sent=0 errs=0 consec=0 backoff=0
    local err_tmp; err_tmp=$(mktemp)

    while [[ ! -f "$stop_file" ]]; do

        if [[ $backoff -gt 0 ]]; then sleep "$backoff"; backoff=0; fi

        local ts hash
        ts=$(ms_now)

        hash=$(_timeout "$CAST_TIMEOUT" cast send \
                --rpc-url     "$L2_RPC_URL" \
                --private-key "$key" \
                --nonce       "$nonce" \
                --gas-price   "${GAS_PRICE_GWEI}gwei" \
                --gas-limit   21000 \
                --async \
                "$RECIPIENT" \
                --value       "0.0001ether" \
                2>"$err_tmp" | tr -d '[:space:]')

        if [[ "$hash" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
            printf '%s\t%d\n' "$hash" "$ts" >> "$out_file"
            nonce=$((nonce + 1)); sent=$((sent + 1)); consec=0

            if [[ $((sent % NONCE_RESYNC_EVERY)) -eq 0 ]]; then
                local chain_nonce
                chain_nonce=$(cast nonce --rpc-url "$L2_RPC_URL" "$addr" 2>/dev/null || echo "$nonce")
                [[ $chain_nonce -gt $nonce ]] && nonce=$chain_nonce
            fi
            continue
        fi

        local stderr_out; stderr_out=$(cat "$err_tmp" 2>/dev/null)
        errs=$((errs + 1)); consec=$((consec + 1))
        printf '[w%d nonce=%d] %s\n' "$worker_id" "$nonce" "$stderr_out" >> "$err_file"

        if echo "$stderr_out" | grep -qiE "nonce too low|nonce.*mismatch|invalid nonce"; then
            nonce=$(cast nonce --rpc-url "$L2_RPC_URL" "$addr" 2>/dev/null || echo "$nonce")
        elif echo "$stderr_out" | grep -qiE "already known|known transaction"; then
            nonce=$((nonce + 1))
        elif echo "$stderr_out" | grep -qiE "replacement.*underpriced|gas price.*too low.*replace"; then
            nonce=$((nonce + 1))
        elif echo "$stderr_out" | grep -qiE "pool.*full|txpool.*full|mempool.*full|too many pending|queue.*full"; then
            backoff=0.5
            nonce=$(cast nonce --rpc-url "$L2_RPC_URL" "$addr" 2>/dev/null || echo "$nonce")
        elif echo "$stderr_out" | grep -qiE "connection refused|EOF|timeout|i/o timeout|dial|no route"; then
            backoff=2
        elif [[ $consec -ge 5 ]]; then
            nonce=$(cast nonce --rpc-url "$L2_RPC_URL" "$addr" 2>/dev/null || echo "$nonce")
            backoff=1; consec=0
        fi
    done

    rm -f "$err_tmp"
    echo "$sent $errs" > "$stats_file"
}

# ── Temp directory (cleaned up on exit) ───────────────────────────────────────
TMP=$(mktemp -d)
STOP_FILE="$TMP/stop"
INTERRUPTED=false
ENGINE_FILE="$TMP/engine.tsv"
touch "$ENGINE_FILE"

_cleanup() { rm -rf "$TMP"; }
trap '_cleanup' EXIT

_graceful_stop() {
    INTERRUPTED=true
    echo ""
    warn "Interrupted — stopping workers and producing partial report..."
    touch "$STOP_FILE" 2>/dev/null || true
}
trap '_graceful_stop' INT TERM

# ── Pre-flight ─────────────────────────────────────────────────────────────────
check_deps cast jq curl python3

step "Pre-flight"
wait_for_rpc "$L2_RPC_URL" "xlayer-node"

CHAIN_ID=$(cast chain-id --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "?")
info "Chain ID : $CHAIN_ID  (expected $EXPECTED_CHAIN_ID)"
info "Duration : ${DURATION}s    Concurrency: $CONCURRENCY workers"
info "Engine API: $AUTH_RPC_URL  (probe: $MEASURE_ENGINE)"
info "Output   : $OUT_FILE"
echo ""

# ── Wait for real-time block rate ──────────────────────────────────────────────
# After a clean restart the node may replay historical blocks from L1 derivation
# much faster than 1/s. Running the test during catch-up gives garbage metrics
# (latency = 0, wrong block counts). Wait here until rate ≤ 7 blocks / 5 s.
step "Waiting for chain to reach real-time block rate"
STAB_TIMEOUT=300
STAB_START=$SECONDS
while true; do
    BN1=$(cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "0")
    sleep 5
    BN2=$(cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "0")
    RATE=$((BN2 - BN1))
    if [[ $RATE -ge 1 && $RATE -le 7 ]]; then
        ok "Block rate stable (${RATE} blocks in 5s)"
        break
    fi
    if [[ $((SECONDS - STAB_START)) -ge $STAB_TIMEOUT ]]; then
        fail "Chain still catching up after ${STAB_TIMEOUT}s (${RATE} blocks/5s) — wait longer then re-run."
        exit 1
    fi
    printf "\r  Catching up: %-4d blocks/5s — waiting (${RATE} blks, %ds elapsed)...   " \
        "$RATE" "$((SECONDS - STAB_START))"
done
echo ""

# Balance check — auto-fund any worker with < 0.1 ETH from worker 0
step "Checking sender balances"
FUNDER_KEY="${SENDER_KEYS[0]}"
for (( i=0; i<CONCURRENCY; i++ )); do
    KEY="${SENDER_KEYS[$i]}"
    ADDR=$(cast wallet address "$KEY" 2>/dev/null || echo "?")
    BAL=$(cast balance --rpc-url "$L2_RPC_URL" "$ADDR" --ether 2>/dev/null || echo "0")
    NOT_ENOUGH=$(python3 -c "print('yes' if float('${BAL}') < 0.1 else 'no')" 2>/dev/null || echo "no")
    if [[ "$NOT_ENOUGH" == "yes" ]]; then
        if [[ $i -eq 0 ]]; then
            fail "Funder (worker 0) $ADDR has only ${BAL} ETH — cannot auto-fund"; exit 1
        fi
        warn "  worker $i : $ADDR has ${BAL} ETH — auto-funding 100 ETH from worker 0..."
        cast send --rpc-url "$L2_RPC_URL" \
            --private-key "$FUNDER_KEY" \
            "$ADDR" --value 100ether \
            --gas-price "${GAS_PRICE_GWEI}gwei" \
            >/dev/null 2>&1 \
            && info "  worker $i : funded ✓" \
            || { fail "Auto-fund failed for worker $i ($ADDR)"; exit 1; }
    else
        info "  worker $i : $ADDR  (${BAL} ETH)"
    fi
done
echo ""

# ── Baseline snapshot ──────────────────────────────────────────────────────────
step "Baseline"
read -r UNSAFE_START SAFE_START FIN_START <<< "$(l2_heads)"
info "Unsafe: $UNSAFE_START  Safe: $SAFE_START  Finalized: $FIN_START"
echo ""

# Bookmark reth log for engine_bridge parsing — only lines from THIS test will be parsed.
# Requires RUST_LOG to include engine_bridge=debug (default in 2-start-node.sh).
RETH_LOG_FILE="${XLAYER_ROOT}/logs/reth/195/reth.log"
LOG_BOOKMARK=0
[[ -f "$RETH_LOG_FILE" ]] && LOG_BOOKMARK=$(wc -l < "$RETH_LOG_FILE" 2>/dev/null || echo 0)

# ── Engine API baseline probe ──────────────────────────────────────────────────
if [[ "$MEASURE_ENGINE" == "true" ]]; then
    step "Engine API baseline probe  (${ENGINE_PROBE_N} FCU pings + ${ENGINE_GP_N} getPayload cycles)"
    run_engine_probe "$ENGINE_FILE" "baseline"
    BL_LINES=$(grep "^baseline:" "$ENGINE_FILE" 2>/dev/null | wc -l | tr -d ' ')
    info "  $BL_LINES measurements recorded"
    echo ""
fi

# Capture block at load start — after baseline probe, before any workers spawn.
# Used so "Blocks produced" and sample window reflect the load phase only.
UNSAFE_LOAD_START=$(cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "$UNSAFE_START")

# ── Spawn TX workers ───────────────────────────────────────────────────────────
touch "$TMP/errors.log"
declare -a WORKER_PIDS=()

step "Starting $CONCURRENCY workers"
for (( i=0; i<CONCURRENCY; i++ )); do
    touch "$TMP/worker_${i}.tsv" "$TMP/worker_${i}.stats"
    (
        _lt_worker "$i" \
            "${SENDER_KEYS[$i]}" \
            "$TMP/worker_${i}.tsv" \
            "$TMP/errors.log" \
            "$TMP/worker_${i}.stats" \
            "$STOP_FILE"
    ) &
    WORKER_PIDS+=($!)
done
info "  ${#WORKER_PIDS[@]} workers running (pids: ${WORKER_PIDS[*]})"
echo ""

# ── Load phase with Engine API sampling ────────────────────────────────────────
step "Running load for ${DURATION}s  (Engine API sampled every 5s)"
LOAD_START=$SECONDS
LAST_ENGINE_SAMPLE=0

while [[ ! -f "$STOP_FILE" && $((SECONDS - LOAD_START)) -lt $DURATION ]]; do
    ELAPSED=$((SECONDS - LOAD_START))
    SENT=$(cat "$TMP"/worker_*.tsv 2>/dev/null | wc -l | tr -d ' ')
    ERRS=$(wc -l < "$TMP/errors.log" 2>/dev/null | tr -d ' ')
    UNSAFE_NOW=$(cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "?")
    printf "\r  [%3ds/%ds]  submitted: %-6s  errors: %-5s  unsafe: %s   " \
        "$ELAPSED" "$DURATION" "$SENT" "$ERRS" "$UNSAFE_NOW"

    # FCU sample every 5 s — runs as a fire-and-forget background call
    if [[ "$MEASURE_ENGINE" == "true" && $((ELAPSED - LAST_ENGINE_SAMPLE)) -ge 5 ]]; then
        LAST_ENGINE_SAMPLE=$ELAPSED
        (
            ENGINE_PROBE_N=3 ENGINE_GP_N=0
            run_engine_probe "$ENGINE_FILE" "load"
        ) &
    fi
    sleep 3
done
printf "\n\n"

# ── Stop TX workers ────────────────────────────────────────────────────────────
touch "$STOP_FILE"
for pid in "${WORKER_PIDS[@]}"; do wait "$pid" 2>/dev/null || true; done

# Capture unsafe head immediately at load end — before receipt polling adds empty blocks.
# Used for block stats so the sample window covers only the load phase.
UNSAFE_LOAD_END=$(cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "$UNSAFE_START")

cat "$TMP"/worker_*.tsv 2>/dev/null | sort -k1,1 -u > "$TMP/all_txs.tsv" || true
TOTAL_SUBMITTED=$(wc -l < "$TMP/all_txs.tsv" | tr -d ' ')
LOAD_ELAPSED=$((SECONDS - LOAD_START))
[[ $LOAD_ELAPSED -eq 0 ]] && LOAD_ELAPSED=1

TOTAL_WORKER_ERRS=0
for (( i=0; i<CONCURRENCY; i++ )); do
    WSTATS=$(cat "$TMP/worker_${i}.stats" 2>/dev/null || echo "0 0")
    WE=$(echo "$WSTATS" | awk '{print $2}')
    TOTAL_WORKER_ERRS=$((TOTAL_WORKER_ERRS + WE))
done

ok "Load done — $TOTAL_SUBMITTED TXs submitted in ${LOAD_ELAPSED}s  ($TOTAL_WORKER_ERRS send-errors)"
echo ""

# ── Engine API post-load probe ─────────────────────────────────────────────────
if [[ "$MEASURE_ENGINE" == "true" ]]; then
    step "Engine API post-load probe  (${ENGINE_PROBE_N} FCU pings)"
    ENGINE_GP_N=0 run_engine_probe "$ENGINE_FILE" "postload"
    echo ""
fi

# ── Parallel receipt polling ───────────────────────────────────────────────────
step "Polling for TX inclusion (${CONFIRM_TIMEOUT}s timeout, ${POLL_BATCH} parallel)"

PENDING=()
while IFS=$'\t' read -r h _; do PENDING+=("$h"); done < "$TMP/all_txs.tsv"
CONFIRMED=0; FAILED=0; POLL_START=$SECONDS

while [[ ${#PENDING[@]} -gt 0 ]]; do
    if [[ $((SECONDS - POLL_START)) -ge $CONFIRM_TIMEOUT ]]; then
        warn "Confirmation timeout — ${#PENDING[@]} TXs unresolved"; break
    fi

    # Use Python eth_getTransactionReceipt — non-blocking, no cast/timeout dependency.
    # Returns null immediately for pending TXs; returns receipt for confirmed ones.
    ROUND_RESULT=$(python3 - "$L2_RPC_URL" "$TMP/all_txs.tsv" \
        "${PENDING[@]}" <<'RCPTPY'
import sys, json, urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed

rpc_url   = sys.argv[1]
tsv_path  = sys.argv[2]
hashes    = sys.argv[3:]

# Build send-timestamp lookup from all_txs.tsv
send_ts = {}
try:
    with open(tsv_path) as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) >= 2:
                send_ts[parts[0]] = parts[1]
except Exception:
    pass

import time
confirm_ms = int(time.time() * 1000)

def fetch_receipt(h):
    body = json.dumps({"jsonrpc":"2.0","id":1,
                       "method":"eth_getTransactionReceipt","params":[h]}).encode()
    req = urllib.request.Request(rpc_url, data=body,
                                  headers={"Content-Type":"application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            result = json.loads(r.read()).get("result")
        if result is None:
            return h, None, None, None
        blk_raw = result.get("blockNumber", "0x0")
        blk = int(blk_raw, 16) if isinstance(blk_raw, str) and blk_raw.startswith("0x") else int(blk_raw or 0)
        status_raw = result.get("status", "0x0")
        status = int(status_raw, 16) if isinstance(status_raw, str) and status_raw.startswith("0x") else int(status_raw or 0)
        return h, blk, status, send_ts.get(h, "0")
    except Exception:
        return h, None, None, None

with ThreadPoolExecutor(max_workers=100) as pool:
    futures = {pool.submit(fetch_receipt, h): h for h in hashes}
    for fut in as_completed(futures):
        h, blk, status, ts = fut.result()
        if blk is not None:
            print(f"OK\t{h}\t{ts}\t{confirm_ms}\t{blk}\t{status}")
        else:
            print(f"PENDING\t{h}")
RCPTPY
)

    STILL_PENDING=()
    while IFS=$'\t' read -r verdict rest; do
        if [[ "$verdict" == "OK" ]]; then
            # rest = hash<TAB>send_ts<TAB>confirm_ms<TAB>blk<TAB>status
            printf '%s\n' "$rest" >> "$TMP/confirmed.tsv"
            STATUS=$(echo "$rest" | awk -F'\t' '{print $5}')
            if [[ "${STATUS:-0}" == "1" ]]; then CONFIRMED=$((CONFIRMED + 1))
            else FAILED=$((FAILED + 1)); fi
        else
            # rest = hash
            STILL_PENDING+=("$rest")
        fi
    done <<< "$ROUND_RESULT"

    if [[ ${#STILL_PENDING[@]} -gt 0 ]]; then PENDING=("${STILL_PENDING[@]}")
    else PENDING=(); fi

    TOTAL_PENDING=${#PENDING[@]}
    printf "\r  confirmed: %-6d  failed: %-4d  pending: %-6d  elapsed: %ds" \
        "$CONFIRMED" "$FAILED" "$TOTAL_PENDING" "$((SECONDS - POLL_START))"
    [[ $TOTAL_PENDING -gt 0 ]] && sleep 2
done
printf "\n\n"
TIMED_OUT=${#PENDING[@]}

# ── Parallel block stats collection ───────────────────────────────────────────
step "Collecting block stats"
UNSAFE_END=$UNSAFE_LOAD_END
BLOCKS_PRODUCED=$((UNSAFE_END - UNSAFE_START))
touch "$TMP/blocks.tsv"

LOAD_BLOCKS=$((UNSAFE_END - UNSAFE_LOAD_START))
if [[ $BLOCKS_PRODUCED -gt 0 ]]; then
    SAMP_START=$((UNSAFE_LOAD_START + 1))
    SAMP_END=$UNSAFE_END
    [[ $LOAD_BLOCKS -gt 120 ]] && SAMP_START=$((UNSAFE_END - 120))

    # Fetch block stats directly via eth_getBlockByNumber — avoids cast block JSON
    # parsing issues (cast may emit warnings before JSON or output text format).
    python3 - "$L2_RPC_URL" "$SAMP_START" "$SAMP_END" >> "$TMP/blocks.tsv" <<'BLKPY'
import sys, json, urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed

rpc_url   = sys.argv[1]
blk_start = int(sys.argv[2])
blk_end   = int(sys.argv[3])

def to_int(v, default=0):
    if v is None: return default
    if isinstance(v, int): return v
    s = str(v).strip()
    if s.startswith("0x") or s.startswith("0X"):
        try: return int(s, 16)
        except: return default
    try: return int(s)
    except:
        try: return int(s, 16)
        except: return default

def fetch_block(blk_num):
    body = json.dumps({
        "jsonrpc": "2.0", "id": blk_num,
        "method": "eth_getBlockByNumber",
        "params": [hex(blk_num), False]
    }).encode()
    req = urllib.request.Request(
        rpc_url, data=body,
        headers={"Content-Type": "application/json"},
        method="POST")
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            d = json.loads(r.read()).get("result") or {}
            num = to_int(d.get("number"))
            ts  = to_int(d.get("timestamp"))
            txc = len(d.get("transactions", []))
            gas = to_int(d.get("gasUsed"))
            if num:
                return f"{num}\t{ts}\t{txc}\t{gas}"
    except Exception:
        pass
    return None

results = {}
with ThreadPoolExecutor(max_workers=20) as pool:
    futures = {pool.submit(fetch_block, n): n for n in range(blk_start, blk_end + 1)}
    for fut in as_completed(futures):
        row = fut.result()
        if row:
            n = futures[fut]
            results[n] = row

for n in sorted(results):
    print(results[n])
BLKPY
fi

# ── Wait for safe head ─────────────────────────────────────────────────────────
if $WAIT_SAFE && [[ $CONFIRMED -gt 0 ]]; then
    # TARGET is max block number seen in confirmed receipts (already in decimal)
    TARGET=$(awk -F'\t' 'BEGIN{m=0} NF==5 && $4+0>m{m=$4+0} END{print m}' \
        "$TMP/confirmed.tsv" 2>/dev/null || echo 0)

    if [[ $TARGET -gt 0 ]]; then
        step "Waiting for safe head ≥ $TARGET (timeout: ${SAFE_WAIT}s)"
        SW_START=$SECONDS
        while true; do
            read -r _ SAFE_NOW _ <<< "$(l2_heads)"
            if [[ $SAFE_NOW -ge $TARGET ]]; then
                ok "Safe head at $SAFE_NOW  (+$((SECONDS - SW_START))s)"; break
            fi
            if [[ $((SECONDS - SW_START)) -ge $SAFE_WAIT ]]; then
                warn "Safe head timeout — at $SAFE_NOW, need $TARGET"; break
            fi
            printf "\r  safe head: %-8d  target: %-8d  elapsed: %ds   " \
                "$SAFE_NOW" "$TARGET" "$((SECONDS - SW_START))"
            sleep 2
        done
        printf "\n\n"
    fi
fi

# ── Parse in-process engine_bridge latencies ───────────────────────────────────
# Reads RUST_LOG=engine_bridge=debug lines added since LOG_BOOKMARK.
# These measure the tokio channel dispatch time (kona → reth) — no HTTP hop.
# Log format: "DEBUG engine_bridge: FCU ok elapsed=252µs payload_id=None"
EB_STATS="{}"
if [[ -f "$RETH_LOG_FILE" ]]; then
    EB_STATS=$(python3 - "$RETH_LOG_FILE" "$LOG_BOOKMARK" <<'EBPY'
import sys, re, json

log_file = sys.argv[1]
bookmark = int(sys.argv[2])

_RE = re.compile(r'^([\d.]+)(.+)$')
def to_ms(v):
    v = v.strip()
    m = _RE.match(v)
    if not m: return 0.0
    num, unit = float(m.group(1)), m.group(2)
    if 'ns' in unit: return num / 1_000_000.0
    if 'ms' in unit: return num
    if unit.endswith('s') and len(unit) > 1: return num / 1000.0  # µs, us
    if unit == 's': return num * 1000.0
    return 0.0

def pct(data, p):
    s = sorted(data)
    return s[min(int(len(s)*p/100), len(s)-1)] if s else 0

def stats(data):
    if not data: return dict(min=0, avg=0, p50=0, p95=0, p99=0, max=0, n=0)
    return dict(min=round(min(data),3), avg=round(sum(data)/len(data),3),
        p50=round(pct(data,50),3), p95=round(pct(data,95),3),
        p99=round(pct(data,99),3), max=round(max(data),3), n=len(data))

fcu = []        # forkchoiceUpdated — no payload (head sync only)
fcu_attrs = []  # forkchoiceUpdated — with payloadAttributes (triggers block build)
new_pay = []    # new_payload — seals a built block into the chain

pat_elapsed = re.compile(r'elapsed=(\S+)')
try:
    with open(log_file, errors='replace') as f:
        for i, line in enumerate(f):
            if i < bookmark: continue
            if 'engine_bridge' not in line: continue
            m = pat_elapsed.search(line)
            if not m: continue
            ms = to_ms(m.group(1))
            if ms <= 0: continue
            if 'new_payload ok' in line:
                new_pay.append(ms)
            elif 'FCU ok' in line:
                if 'payload_id=Some' in line:
                    fcu_attrs.append(ms)
                else:
                    fcu.append(ms)
except Exception:
    pass

print(json.dumps({"fcu": stats(fcu), "fcu_attrs": stats(fcu_attrs), "new_pay": stats(new_pay)}))
EBPY
) || EB_STATS="{}"
fi

# ── Final heads ────────────────────────────────────────────────────────────────
read -r UNSAFE_FINAL SAFE_FINAL FIN_FINAL <<< "$(l2_heads)"

# ── Metrics — computed in Python ───────────────────────────────────────────────
METRICS=$(python3 - \
    "$TMP/confirmed.tsv" \
    "$TMP/blocks.tsv" \
    "$ENGINE_FILE" \
    "$TOTAL_SUBMITTED" "$CONFIRMED" "$FAILED" "$TIMED_OUT" "$TOTAL_WORKER_ERRS" \
    "$LOAD_ELAPSED" \
    "$UNSAFE_START" "$UNSAFE_FINAL" \
    "$SAFE_START"   "$SAFE_FINAL" \
    "$FIN_START"    "$FIN_FINAL" \
    "$UNSAFE_LOAD_START" "$UNSAFE_LOAD_END" \
<<'PYEOF'
import sys, json, statistics

conf_f, blk_f, eng_f = sys.argv[1], sys.argv[2], sys.argv[3]
submitted  = int(sys.argv[4])
confirmed  = int(sys.argv[5])
failed     = int(sys.argv[6])
timed_out  = int(sys.argv[7])
send_errs  = int(sys.argv[8])
elapsed    = int(sys.argv[9])
ubs, ube   = int(sys.argv[10]), int(sys.argv[11])
ss,  se    = int(sys.argv[12]), int(sys.argv[13])
fs,  fe    = int(sys.argv[14]), int(sys.argv[15])
uls, ule   = int(sys.argv[16]), int(sys.argv[17])  # load-phase block window

def pct(data, p):
    if not data: return 0
    s = sorted(data)
    return s[min(int(len(s)*p/100), len(s)-1)]

def stats(data):
    if not data: return dict(min=0, avg=0, p50=0, p95=0, p99=0, max=0, n=0)
    return dict(
        min=round(min(data), 2), avg=round(sum(data)/len(data), 2),
        p50=round(pct(data, 50), 2), p95=round(pct(data, 95), 2),
        p99=round(pct(data, 99), 2), max=round(max(data), 2),
        n=len(data))

# ── Block stats (read first — blk_ts_ms needed for TX latency join) ────────────
rows = []
blk_ts_ms = {}  # blk_num → unix timestamp in milliseconds
try:
    for line in open(blk_f):
        p = line.strip().split('\t')
        if len(p) == 4:
            rows.append((int(p[0]), int(p[1]), int(p[2]), int(p[3])))
            blk_ts_ms[int(p[0])] = int(p[1]) * 1000
except FileNotFoundError: pass

rows.sort()
avg_bt = avg_txb = peak_txb = avg_gas = 0
no_gaps = True

if len(rows) > 1:
    ts_deltas = [rows[i+1][1] - rows[i][1] for i in range(len(rows)-1)]
    avg_bt    = round(sum(ts_deltas)/len(ts_deltas), 2)
    txcounts  = [r[2] for r in rows]
    avg_txb   = round(sum(txcounts)/len(txcounts), 1)
    peak_txb  = max(txcounts)
    avg_gas   = int(sum(r[3] for r in rows)/len(rows))
    nums      = [r[0] for r in rows]
    no_gaps   = all(nums[i+1] == nums[i]+1 for i in range(len(nums)-1))

# ── TX latencies — joined on block number ──────────────────────────────────────
# confirmed.tsv: hash | send_ts_ms | confirm_ms(ignored) | blk | status
# Latency = block.timestamp*1000 − send_ts_ms  (accurate to 1-second block granularity)
latencies, statuses = [], []
try:
    for line in open(conf_f):
        p = line.strip().split('\t')
        if len(p) == 5:
            blk_num = int(p[3])
            send_ms = int(p[1])
            if blk_num in blk_ts_ms:
                lat = blk_ts_ms[blk_num] - send_ms
                if lat >= 0: latencies.append(lat)
            statuses.append(p[4])
except FileNotFoundError: pass

lat = stats(latencies)
all_ok   = all(s == '1' for s in statuses)
fail_cnt = statuses.count('0')

# ── Engine API latencies ───────────────────────────────────────────────────────
eng = {"baseline:fcu": [], "baseline:fcu_attrs": [], "baseline:getpayload": [],
       "load:fcu": [], "postload:fcu": []}
try:
    for line in open(eng_f):
        parts = line.strip().split('\t')
        if len(parts) == 2:
            key, val = parts[0], float(parts[1])
            if val > 0 and key in eng:
                eng[key].append(val)
except FileNotFoundError: pass

eng_stats = {k: stats(v) for k, v in eng.items()}

# ── Rates ──────────────────────────────────────────────────────────────────────
sub_rate = round(submitted / max(elapsed, 1), 1)
eff_tps  = round(confirmed / max(elapsed, 1), 1)
incl_pct = round(100.0 * confirmed / max(submitted, 1), 1)

safe_lag_start    = ubs - ss
safe_lag_end      = ube - se
safe_lag_increase = safe_lag_end - safe_lag_start

print(json.dumps({
    "submitted": submitted, "confirmed": confirmed,
    "failed": failed, "timed_out": timed_out, "send_errs": send_errs,
    "sub_rate": sub_rate, "eff_tps": eff_tps, "incl_pct": incl_pct,
    "lat_min": lat['min'], "lat_avg": lat['avg'],
    "lat_p50": lat['p50'], "lat_p95": lat['p95'],
    "lat_p99": lat['p99'], "lat_max": lat['max'],
    "all_ok": all_ok, "fail_cnt": fail_cnt,
    "blocks": ule - uls,  # load-phase blocks only (excludes baseline probe period)
    "avg_bt": avg_bt, "avg_txb": avg_txb, "peak_txb": peak_txb,
    "avg_gas": avg_gas, "no_gaps": no_gaps,
    "ubs": ubs, "ube": ube, "ss": ss, "se": se, "fs": fs, "fe": fe,
    "safe_lag": safe_lag_end,
    "safe_lag_start": safe_lag_start,
    "safe_lag_end": safe_lag_end,
    "safe_lag_increase": safe_lag_increase,
    "fin_lag": ube - fe,
    "eng": eng_stats,
}))
PYEOF
)

_m() { echo "$METRICS" | python3 -c \
    "import sys,json; d=json.load(sys.stdin); print(d.get('$1','?'))" 2>/dev/null || echo "?"; }
_me() {
    # _me KEY STAT — e.g. _me baseline:fcu p50
    echo "$METRICS" | python3 -c \
    "import sys,json; d=json.load(sys.stdin); print(d.get('eng',{}).get('$1',{}).get('$2','?'))" \
    2>/dev/null || echo "?"
}
_meb() {
    # _meb TYPE STAT — e.g. _meb fcu_attrs p50  (engine_bridge in-process channel stats)
    echo "$EB_STATS" | python3 -c \
    "import sys,json; d=json.load(sys.stdin); print(d.get('$1',{}).get('$2','?'))" \
    2>/dev/null || echo "?"
}

# ── Assertions ────────────────────────────────────────────────────────────────
ASSERT_PASS=0 ASSERT_FAIL=0
declare -a ASSERT_LINES=()

assert() {
    local label="$1" result="$2" note="${3:-}"
    local tag
    if [[ "$result" == "true" ]]; then
        tag="$(printf '\033[32m[PASS]\033[0m')"; ASSERT_PASS=$((ASSERT_PASS + 1))
    else
        tag="$(printf '\033[31m[FAIL]\033[0m')"; ASSERT_FAIL=$((ASSERT_FAIL + 1))
    fi
    ASSERT_LINES+=("  $tag $label${note:+  ($note)}")
}
py_cmp() { python3 -c "print('true' if ($1) else 'false')" 2>/dev/null || echo "false"; }

assert "Chain ID = $EXPECTED_CHAIN_ID" \
    "$([[ "$CHAIN_ID" == "$EXPECTED_CHAIN_ID" ]] && echo true || echo false)"
assert "Unsafe head advanced  (+$((UNSAFE_FINAL - UNSAFE_START)) blocks)" \
    "$([[ $UNSAFE_FINAL -gt $UNSAFE_START ]] && echo true || echo false)"
assert "Safe head advanced  (+$((SAFE_FINAL - SAFE_START)) blocks)" \
    "$([[ $SAFE_FINAL -gt $SAFE_START ]] && echo true || echo false)" "batcher healthy"
assert "Finalized head tracked  (lag: $(_m fin_lag) blocks)" \
    "$(py_cmp "int('$(_m fin_lag)') < 1500")" "finalization needs L1 finality + dispute game; lags by design in devnet"
assert "All confirmed TX receipts status = 1" \
    "$([[ "$(_m all_ok)" == "True" ]] && echo true || echo false)" \
    "$(_m fail_cnt) with bad status"
assert "TX inclusion rate ≥ 90%  (got $(_m incl_pct)%)" \
    "$(py_cmp "float('$(_m incl_pct)') >= 90")"
assert "Send-error rate < 5%  (got $TOTAL_WORKER_ERRS errors / $TOTAL_SUBMITTED sends)" \
    "$(py_cmp "float($TOTAL_WORKER_ERRS) / max($TOTAL_SUBMITTED,1) < 0.05")"
assert "Avg block time ≤ 1.5s  (got $(_m avg_bt)s)" \
    "$(py_cmp "0 < float('$(_m avg_bt)') <= 1.5")"
assert "No block number gaps in test window" \
    "$([[ "$(_m no_gaps)" == "True" ]] && echo true || echo false)"
assert "Safe lag increase ≤ 50 blocks  (start: $(_m safe_lag_start)  end: $(_m safe_lag_end)  Δ: +$(_m safe_lag_increase))" \
    "$(py_cmp "int('$(_m safe_lag_increase)') <= 50")" "batcher cadence; pre-existing backlog ignored"

if [[ "$MEASURE_ENGINE" == "true" ]]; then
    BL_FCU_N=$(_me "baseline:fcu" n)
    assert "Engine API responsive  (${BL_FCU_N} FCU calls succeeded)" \
        "$(py_cmp "int('${BL_FCU_N}') > 0")"
fi

TOTAL_ASSERTS=$((ASSERT_PASS + ASSERT_FAIL))

# ── Report ─────────────────────────────────────────────────────────────────────
RULE="══════════════════════════════════════════════════════════════"
HRULE="──────────────────────────────────────────────────────────────"

[[ $ASSERT_FAIL -eq 0 ]] \
    && VERDICT="\033[32m PASS \033[0m" VERDICT_PLAIN="PASS" \
    || VERDICT="\033[31m FAIL \033[0m" VERDICT_PLAIN="FAIL"

{
echo ""
echo "$RULE"
printf "  xlayer-node Load Test Report%s\n" "$($INTERRUPTED && echo '  [PARTIAL — interrupted]' || true)"
printf "  Duration: %ds    Concurrency: %d workers    Chain: %s\n" \
    "$LOAD_ELAPSED" "$CONCURRENCY" "$CHAIN_ID"
printf "  Date:     %s\n" "$(date)"
echo "$RULE"
echo ""
printf "  %-36s\n" "THROUGHPUT"
printf "  %-40s %s\n"  "TXs submitted"               "$(_m submitted)"
printf "  %-40s %s\n"  "TXs confirmed (unsafe)"       "$(_m confirmed)  ($(_m incl_pct)%)"
printf "  %-40s %s\n"  "TXs with failed receipt"      "$(_m fail_cnt)"
printf "  %-40s %s\n"  "TXs timed-out / unresolved"   "$TIMED_OUT"
printf "  %-40s %s\n"  "Send-errors (RPC / nonce)"    "$TOTAL_WORKER_ERRS"
printf "  %-40s %s\n"  "Submission rate"               "$(_m sub_rate) TX/s"
printf "  %-40s %s\n"  "Effective TPS (unsafe)"        "$(_m eff_tps) TX/s"
echo ""
printf "  %-36s\n" "TX INCLUSION LATENCY  (send_ts → block.timestamp, ms)"
printf "  %-40s %s\n"  "Min"   "$(_m lat_min)"
printf "  %-40s %s\n"  "Avg"   "$(_m lat_avg)"
printf "  %-40s %s\n"  "p50"   "$(_m lat_p50)"
printf "  %-40s %s\n"  "p95"   "$(_m lat_p95)"
printf "  %-40s %s\n"  "p99"   "$(_m lat_p99)"
printf "  %-40s %s\n"  "Max"   "$(_m lat_max)"
echo ""
printf "  %-36s\n" "BLOCK PRODUCTION"
printf "  %-40s %s\n"  "Blocks produced"              "$(_m blocks)"
printf "  %-40s %s\n"  "Avg block time"                "$(_m avg_bt) s"
printf "  %-40s %s\n"  "Avg TXs / block"               "$(_m avg_txb)"
printf "  %-40s %s\n"  "Peak TXs / block"              "$(_m peak_txb)"
printf "  %-40s %s\n"  "Avg gas / block"               "$(_m avg_gas)"
echo ""
printf "  %-36s\n" "L2 HEADS  (start → end)"
printf "  %-40s %d → %d  (+%d)\n" \
    "Unsafe"    "$UNSAFE_START" "$UNSAFE_FINAL" "$((UNSAFE_FINAL - UNSAFE_START))"
printf "  %-40s %d → %d  (+%d)  lag: %s blks  (Δ during test: %+d)\n" \
    "Safe"      "$SAFE_START"   "$SAFE_FINAL"   "$((SAFE_FINAL   - SAFE_START))"  "$(_m safe_lag_end)" "$(_m safe_lag_increase)"
printf "  %-40s %d → %d  (+%d)  lag: %s blocks\n" \
    "Finalized" "$FIN_START"    "$FIN_FINAL"    "$((FIN_FINAL    - FIN_START))"   "$(_m fin_lag)"
echo ""

# HTTP Engine API probe section — external HTTP round-trip to authrpc
if [[ "$MEASURE_ENGINE" == "true" ]]; then
    printf "  %-36s\n" "HTTP ENGINE API PROBE  (ms, test-host → authrpc)"
    printf "  %-2s  %-26s  %7s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
        "" "Call" "min" "avg" "p50" "p95" "p99" "max" "n"
    printf "  %-2s  %-26s  %7s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
        "" "----" "---" "---" "---" "---" "---" "---" "-"
    for key in "baseline:fcu" "baseline:fcu_attrs" "baseline:getpayload" \
               "load:fcu" "postload:fcu"; do
        N=$(_me "$key" n)
        [[ "$N" == "0" || "$N" == "?" ]] && continue
        case "$key" in
            baseline:fcu)        LABEL="FCU (baseline)" ;;
            baseline:fcu_attrs)  LABEL="FCU+attrs (baseline)" ;;
            baseline:getpayload) LABEL="getPayload (baseline)" ;;
            load:fcu)            LABEL="FCU (under load)" ;;
            postload:fcu)        LABEL="FCU (post-load)" ;;
        esac
        printf "  %-2s  %-26s  %7s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
            "" "$LABEL" \
            "$(_me "$key" min)" "$(_me "$key" avg)" "$(_me "$key" p50)" \
            "$(_me "$key" p95)" "$(_me "$key" p99)" "$(_me "$key" max)" "$N"
    done
    echo ""
    printf "  %-40s %s\n" "Endpoint" "$AUTH_RPC_URL"
    printf "  %-40s %s\n" "Note" "External HTTP probe; includes local loopback"
    printf "  %-40s %s\n" ""     "latency. NOT the kona→reth channel time."
    printf "  %-40s %s\n" ""     "See ENGINE BRIDGE section below for that."
    echo ""
fi

# In-process engine_bridge section — parsed from RUST_LOG=engine_bridge=debug
EB_FCU_N=$(_meb fcu n)
EB_FA_N=$(_meb fcu_attrs n)
EB_NP_N=$(_meb new_pay n)
EB_TOTAL=$(python3 -c "
def v(x):
    try: return int(x)
    except: return 0
print(v('${EB_FCU_N}') + v('${EB_FA_N}') + v('${EB_NP_N}'))
" 2>/dev/null || echo 0)
if [[ $EB_TOTAL -gt 0 ]]; then
    printf "  %-36s\n" "ENGINE BRIDGE LATENCY  (kona→reth in-process, ms)"
    printf "  %-2s  %-28s  %7s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
        "" "Call" "min" "avg" "p50" "p95" "p99" "max" "n"
    printf "  %-2s  %-28s  %7s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
        "" "----" "---" "---" "---" "---" "---" "---" "-"
    if [[ "$EB_FCU_N" != "0" && "$EB_FCU_N" != "?" ]]; then
        printf "  %-2s  %-28s  %7s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
            "" "FCU (head update)" \
            "$(_meb fcu min)" "$(_meb fcu avg)" "$(_meb fcu p50)" \
            "$(_meb fcu p95)" "$(_meb fcu p99)" "$(_meb fcu max)" "$EB_FCU_N"
    fi
    if [[ "$EB_FA_N" != "0" && "$EB_FA_N" != "?" ]]; then
        printf "  %-2s  %-28s  %7s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
            "" "FCU+attrs (starts build)" \
            "$(_meb fcu_attrs min)" "$(_meb fcu_attrs avg)" "$(_meb fcu_attrs p50)" \
            "$(_meb fcu_attrs p95)" "$(_meb fcu_attrs p99)" "$(_meb fcu_attrs max)" "$EB_FA_N"
    fi
    if [[ "$EB_NP_N" != "0" && "$EB_NP_N" != "?" ]]; then
        printf "  %-2s  %-28s  %7s  %7s  %7s  %7s  %7s  %7s  %5s\n" \
            "" "new_payload (seal block)" \
            "$(_meb new_pay min)" "$(_meb new_pay avg)" "$(_meb new_pay p50)" \
            "$(_meb new_pay p95)" "$(_meb new_pay p99)" "$(_meb new_pay max)" "$EB_NP_N"
    fi
    echo ""
    printf "  %-40s %s\n" "Source" "$RETH_LOG_FILE"
    printf "  %-40s %s\n" "Lines" "${LOG_BOOKMARK}+ (from test start to EOF)"
    printf "  %-40s %s\n" "What" "tokio channel time: kona dispatches FCU/"
    printf "  %-40s %s\n" ""     "new_payload → reth Engine handler returns."
    printf "  %-40s %s\n" ""     "No HTTP; sub-millisecond in unified node."
    echo ""
fi

echo "  $HRULE"
printf "  ASSERTIONS  (%d / %d passed)\n" "$ASSERT_PASS" "$TOTAL_ASSERTS"
echo "  $HRULE"
for line in "${ASSERT_LINES[@]}"; do echo "$line"; done
echo ""
echo "  $HRULE"
printf "  KEY METRICS SUMMARY\n"
echo "  $HRULE"
printf "  %-44s %-15s %s\n" "Metric" "Value" "How captured"
printf "  %-44s %-15s %s\n" "──────" "─────" "────────────"
printf "  %-44s %-15s %s\n" \
    "Effective TPS (unsafe)" \
    "$(_m eff_tps) TX/s" \
    "confirmed TXs ÷ load duration (wall-clock)"
printf "  %-44s %-15s %s\n" \
    "TX inclusion latency p50" \
    "$(_m lat_p50) ms" \
    "send_ts_ms → block.timestamp*1000 (joined on blk)"
printf "  %-44s %-15s %s\n" \
    "TX inclusion latency p99" \
    "$(_m lat_p99) ms" \
    "same; 99th percentile"
printf "  %-44s %-15s %s\n" \
    "Avg block time" \
    "$(_m avg_bt) s" \
    "block.timestamp deltas, load-phase window only"
printf "  %-44s %-15s %s\n" \
    "Peak TXs / block" \
    "$(_m peak_txb)" \
    "max tx count across sampled blocks"
printf "  %-44s %-15s %s\n" \
    "Safe lag increase during test" \
    "+$(_m safe_lag_increase) blks" \
    "lag_end($(_m safe_lag_end)) − lag_start($(_m safe_lag_start))"
EB_FA_AVG=$(_meb fcu_attrs avg)
EB_NP_AVG=$(_meb new_pay avg)
if [[ "$EB_FA_AVG" != "0" && "$EB_FA_AVG" != "?" ]]; then
    printf "  %-44s %-15s %s\n" \
        "FCU+attrs latency avg (kona→reth)" \
        "${EB_FA_AVG} ms" \
        "engine_bridge=debug log, elapsed= field"
fi
if [[ "$EB_NP_AVG" != "0" && "$EB_NP_AVG" != "?" ]]; then
    printf "  %-44s %-15s %s\n" \
        "new_payload latency avg (kona→reth)" \
        "${EB_NP_AVG} ms" \
        "engine_bridge=debug log, elapsed= field"
fi
echo ""
echo "  $HRULE"
} | tee "$OUT_FILE"

printf "  RESULT: ${VERDICT}   (%d/%d assertions passed)\n" "$ASSERT_PASS" "$TOTAL_ASSERTS"
printf "  RESULT: %s   (%d/%d assertions passed)\n" \
    "$VERDICT_PLAIN" "$ASSERT_PASS" "$TOTAL_ASSERTS" >> "$OUT_FILE"

printf "  Report saved: %s\n" "$OUT_FILE"
{ echo "  $RULE"; echo ""; } | tee -a "$OUT_FILE"

if [[ -s "$TMP/errors.log" ]]; then
    ERROR_LOG="${OUT_FILE%.txt}-errors.log"
    cp "$TMP/errors.log" "$ERROR_LOG"
    info "  Send-error log: $ERROR_LOG  ($(wc -l < "$ERROR_LOG" | tr -d ' ') lines)"
fi

[[ $ASSERT_FAIL -eq 0 ]]
