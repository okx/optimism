#!/usr/bin/env bash
# simple-bench.sh — quick TPS + TX-latency + Engine API timing breakdown
#
# Usage: ./scripts/devnet/simple-bench.sh [--duration 60] [--workers 6]
#
# Produces identical report fields to xlayer-toolkit/devnet/simple-bench.sh so
# the two stacks can be compared side-by-side.
#
# ENGINE API BREAKDOWN — two sources measured per call type:
#
#   kona (CL)       kona EngineClient round-trip: send → receive ACK
#                   source: logs/xlayer-node.log  (stdout, ANSI-coded, no target prefix)
#   engine-bridge   ChannelEngineClient dispatch to reth (EL) and back
#                   source: logs/reth/195/reth.log (engine_bridge target, debug)
#   reth (EL)       reth on_fcu / on_new_payload internal processing time
#                   source: logs/reth/195/reth.log (engine::tree target, info)
#
#   engine-bridge overhead = kona (CL) total − reth (EL)
#
# Call types measured: FCU (no attrs) · FCU+attrs · new_payload · get_payload

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

# ── Defaults ───────────────────────────────────────────────────────────────────
DURATION=60
CONCURRENCY=6

while [[ $# -gt 0 ]]; do
    case "$1" in
        --duration)  DURATION="$2";    shift 2 ;;
        --workers)   CONCURRENCY="$2"; shift 2 ;;
        --help|-h)   grep '^#' "$0" | sed 's/^# \?//'; exit 0 ;;
        *)           warn "Unknown flag: $1 (ignored)"; shift ;;
    esac
done

# ── Config ─────────────────────────────────────────────────────────────────────
SENDER_KEYS=(
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926b"
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"
    "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e"
)
RECIPIENT="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
GAS_PRICE_GWEI=1
L2_RPC="${L2_RPC_URL:-http://localhost:8123}"
RETH_LOG="${XLAYER_ROOT}/logs/reth/195/reth.log"
NODE_LOG="${XLAYER_ROOT}/logs/xlayer-node.log"

[[ $CONCURRENCY -gt ${#SENDER_KEYS[@]} ]] && CONCURRENCY=${#SENDER_KEYS[@]}

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
ALL_TXS="$TMP/all_txs.tsv"
STOP_FILE="$TMP/stop"
touch "$ALL_TXS"

ms_now() { python3 -c 'import time; print(int(time.time()*1000))'; }

# ── Wait for chain ─────────────────────────────────────────────────────────────
step "Waiting for L2 blocks..."
for _ in $(seq 1 30); do
    BN=$(cast bn --rpc-url "$L2_RPC" 2>/dev/null || echo 0)
    [[ $BN -gt 0 ]] && break
    sleep 1
done
[[ $BN -gt 0 ]] || { fail "L2 RPC not producing blocks — is xlayer-node running?"; exit 1; }
ok "L2 responding at block $BN"

# ── Bookmark logs ──────────────────────────────────────────────────────────────
RETH_BOOKMARK=0
NODE_BOOKMARK=0
[[ -f "$RETH_LOG" ]] && RETH_BOOKMARK=$(wc -l < "$RETH_LOG")
[[ -f "$NODE_LOG" ]] && NODE_BOOKMARK=$(wc -l < "$NODE_LOG")

UNSAFE_START=$(cast bn --rpc-url "$L2_RPC" 2>/dev/null || echo 0)

# ── Worker: send ETH transfers until STOP_FILE ─────────────────────────────────
_worker() {
    local key="$1" out="$2"
    set +e
    local addr; addr=$(cast wallet address "$key" 2>/dev/null) || return
    local nonce; nonce=$(cast nonce --rpc-url "$L2_RPC" "$addr" 2>/dev/null || echo 0)

    while [[ ! -f "$STOP_FILE" ]]; do
        local ts hash
        ts=$(ms_now)
        hash=$(cast send \
            --rpc-url     "$L2_RPC" \
            --private-key "$key" \
            --nonce       "$nonce" \
            --gas-price   "${GAS_PRICE_GWEI}gwei" \
            --gas-limit   21000 \
            --async \
            "$RECIPIENT" \
            --value "0.0001ether" 2>/dev/null | tr -d '[:space:]')

        if [[ "$hash" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
            printf '%s\t%d\n' "$hash" "$ts" >> "$out"
            nonce=$((nonce + 1))
        else
            sleep 0.1
            local cn; cn=$(cast nonce --rpc-url "$L2_RPC" "$addr" 2>/dev/null || echo "$nonce")
            [[ $cn -gt $nonce ]] && nonce=$cn
        fi
    done
}

# ── Launch workers ─────────────────────────────────────────────────────────────
step "Running $CONCURRENCY workers for ${DURATION}s..."
T_START=$(ms_now)
for ((i=0; i<CONCURRENCY; i++)); do
    _worker "${SENDER_KEYS[$i]}" "$ALL_TXS" &
done

sleep "$DURATION"
touch "$STOP_FILE"
wait
T_END=$(ms_now)
LOAD_ELAPSED=$(( (T_END - T_START) / 1000 ))
UNSAFE_LOAD_END=$(cast bn --rpc-url "$L2_RPC" 2>/dev/null || echo 0)

# ── Poll receipts ──────────────────────────────────────────────────────────────
step "Collecting receipts and block timestamps..."
CONFIRMED_FILE="$TMP/confirmed.tsv"
BLOCKS_FILE="$TMP/blocks.tsv"
touch "$CONFIRMED_FILE" "$BLOCKS_FILE"

python3 - "$ALL_TXS" "$L2_RPC" "$CONFIRMED_FILE" "$BLOCKS_FILE" <<'PYRECEIPT'
import sys, json, urllib.request, time

all_txs_file, rpc_url, confirmed_file, blocks_file = sys.argv[1:5]

def rpc(method, params):
    req = json.dumps({"jsonrpc":"2.0","id":1,"method":method,"params":params}).encode()
    _r = urllib.request.Request(rpc_url, data=req,
        headers={"Content-Type":"application/json"}, method="POST")
    with urllib.request.urlopen(_r, timeout=5) as r:
        return json.loads(r.read())["result"]

txs = {}
try:
    with open(all_txs_file) as f:
        for line in f:
            p = line.strip().split('\t')
            if len(p) == 2: txs[p[0]] = int(p[1])
except: pass

if not txs:
    sys.exit(0)

deadline = time.time() + 10
pending = set(txs.keys())
confirmed = {}
while pending and time.time() < deadline:
    for h in list(pending)[:50]:
        try:
            r = rpc("eth_getTransactionReceipt", [h])
            if r and r.get("blockNumber"):
                confirmed[h] = {"send_ms": txs[h], "block": int(r["blockNumber"], 16)}
                pending.discard(h)
        except: pass
    if pending: time.sleep(0.5)

block_nums = set()
with open(confirmed_file, "w") as f:
    for h, d in confirmed.items():
        f.write(f"{h}\t{d['send_ms']}\t{d['block']}\n")
        block_nums.add(d["block"])

with open(blocks_file, "w") as f:
    for bn in sorted(block_nums):
        try:
            blk = rpc("eth_getBlockByNumber", [hex(bn), False])
            if blk:
                f.write(f"{bn}\t{int(blk['timestamp'], 16) * 1000}\n")
        except: pass
PYRECEIPT

# ── Core metrics ───────────────────────────────────────────────────────────────
METRICS=$(python3 - "$CONFIRMED_FILE" "$BLOCKS_FILE" "$LOAD_ELAPSED" "$L2_RPC" <<'PYMETRICS'
import sys, json, urllib.request

conf_file, blk_file, elapsed, rpc_url = sys.argv[1:5]
elapsed = int(elapsed)

def rpc(m, p):
    req = json.dumps({"jsonrpc":"2.0","id":1,"method":m,"params":p}).encode()
    _r = urllib.request.Request(rpc_url, data=req,
        headers={"Content-Type":"application/json"}, method="POST")
    with urllib.request.urlopen(_r, timeout=5) as r:
        return json.loads(r.read())["result"]

confirmed = {}
try:
    with open(conf_file) as f:
        for line in f:
            p = line.strip().split('\t')
            if len(p) == 3: confirmed[p[0]] = {"send_ms": int(p[1]), "block": int(p[2])}
except: pass

blk_ts = {}
try:
    with open(blk_file) as f:
        for line in f:
            p = line.strip().split('\t')
            if len(p) == 2: blk_ts[int(p[0])] = int(p[1])
except: pass

def pct(data, p):
    s = sorted(data)
    return s[min(int(len(s)*p/100), len(s)-1)] if s else 0

latencies = [blk_ts[d["block"]] - d["send_ms"]
             for d in confirmed.values()
             if d["block"] in blk_ts and 0 < blk_ts[d["block"]] - d["send_ms"] < 60000]

blks = sorted(blk_ts.keys())
avg_bt = round((blk_ts[blks[-1]] - blk_ts[blks[0]]) / 1000 / max(len(blks)-1, 1), 2) if len(blks) >= 2 else "?"

print(json.dumps({
    "tps":  round(len(confirmed) / max(elapsed, 1), 1),
    "p50":  pct(latencies, 50),
    "p99":  pct(latencies, 99),
    "avg_bt": avg_bt,
    "n_conf": len(confirmed),
    "n_blks": len(blk_ts),
}))
PYMETRICS
)

# ── Parse Engine API timings from log files ────────────────────────────────────
ENG_STATS=$(python3 - "$RETH_LOG" "$RETH_BOOKMARK" "$NODE_LOG" "$NODE_BOOKMARK" <<'PYFCU'
import sys, re, json, os

reth_log, reth_bm, node_log, node_bm = sys.argv[1:5]
reth_bm = int(reth_bm); node_bm = int(node_bm)

_NUM = re.compile(r'^([\d.]+)(.+)$')
def to_ms(v):
    v = v.strip().rstrip(')')
    m = _NUM.match(v)
    if not m: return 0.0
    num, unit = float(m.group(1)), m.group(2)
    if 'ns' in unit:  return num / 1_000_000.0
    if 'µs' in unit or 'us' in unit: return num / 1000.0
    if 'ms' in unit:  return num
    if unit == 's':   return num * 1000.0
    return 0.0

def pct(data, p):
    s = sorted(data)
    return s[min(int(len(s)*p/100), len(s)-1)] if s else 0

def stats(data):
    if not data: return None
    return dict(p50=round(pct(data,50),3), avg=round(sum(data)/len(data),3), n=len(data))

# engine-bridge (ChannelEngineClient) — engine_bridge target, debug level
# reth (EL)                            — engine::tree target, info level
bridge  = {"fcu": [], "fcu_attrs": [], "new_pay": [], "get_pay": []}
reth_el = {"fcu": [], "fcu_attrs": [], "new_pay": []}

pat_el = re.compile(r'elapsed=([\d.]+[a-zµ]+)')
if os.path.isfile(reth_log):
    try:
        with open(reth_log, errors='replace') as f:
            for i, line in enumerate(f):
                if i < reth_bm: continue
                m = pat_el.search(line)
                if not m: continue
                ms = to_ms(m.group(1))
                if ms <= 0: continue
                if 'engine_bridge' in line:
                    if   'new_payload ok' in line:              bridge["new_pay"].append(ms)
                    elif 'get_payload ok' in line:              bridge["get_pay"].append(ms)
                    elif 'FCU ok' in line and 'payload_id=Some' in line: bridge["fcu_attrs"].append(ms)
                    elif 'FCU ok' in line:                      bridge["fcu"].append(ms)
                elif 'engine::tree' in line:
                    if   'new_payload reth ok' in line:         reth_el["new_pay"].append(ms)
                    elif 'FCU reth ok' in line and 'attrs=true' in line:  reth_el["fcu_attrs"].append(ms)
                    elif 'FCU reth ok' in line:                 reth_el["fcu"].append(ms)
    except: pass

# kona (CL) — stdout log, no target prefix, has ANSI codes
# "block build started fcu_duration=..." → FCU+attrs
# "FCU ok fcu_duration=..."              → FCU no-attrs (SynchronizeTask)
# "Inserted new unsafe block ... insert_duration=..." → new_payload
# "get_payload ok get_payload_duration=..." → get_payload
kona_cl = {"fcu": [], "fcu_attrs": [], "new_pay": [], "get_pay": []}
pat_fcu = re.compile(r'fcu_duration=([\d.]+[a-zµ]+)')
pat_ins = re.compile(r'insert_duration=([\d.]+[a-zµ]+)')
pat_gp  = re.compile(r'get_payload_duration=([\d.]+[a-zµ]+)')
_ANSI   = re.compile(r'\x1b\[[0-9;]*[mK]')
if os.path.isfile(node_log):
    try:
        with open(node_log, errors='replace') as f:
            for i, line in enumerate(f):
                if i < node_bm: continue
                line = _ANSI.sub('', line)
                if 'block build started' in line:
                    m = pat_fcu.search(line)
                    if m: kona_cl["fcu_attrs"].append(to_ms(m.group(1)))
                elif 'FCU ok' in line and 'engine_bridge' not in line:
                    m = pat_fcu.search(line)
                    if m: kona_cl["fcu"].append(to_ms(m.group(1)))
                elif 'Inserted new unsafe block' in line:
                    m = pat_ins.search(line)
                    if m: kona_cl["new_pay"].append(to_ms(m.group(1)))
                elif 'get_payload ok' in line and 'engine_bridge' not in line:
                    m = pat_gp.search(line)
                    if m: kona_cl["get_pay"].append(to_ms(m.group(1)))
    except: pass

print(json.dumps({k: {call: stats(v) for call, v in d.items()} for k, d in
                  {"kona": kona_cl, "bridge": bridge, "reth": reth_el}.items()}))
PYFCU
)

# ── Print report ───────────────────────────────────────────────────────────────
python3 - "$METRICS" "$ENG_STATS" "$DURATION" "$CONCURRENCY" <<'PYPRINT'
import sys, json

m    = json.loads(sys.argv[1])
eng  = json.loads(sys.argv[2])
dur  = int(sys.argv[3])
conc = int(sys.argv[4])

def ms(d, key="p50"):
    return d.get(key) if d else None

def fmt(v):
    return f"{v:.3f} ms" if v is not None else "N/A"

def diff(total, part):
    if total is None or part is None: return "N/A"
    return fmt(max(0.0, total - part))

kona_cl = eng.get("kona", {})
reth_el = eng.get("reth", {})

# total = kona (CL) timer: kona's full round-trip through engine-bridge to reth (EL) and back
kona_total = {
    "fcu":       ms(kona_cl.get("fcu")),
    "fcu_attrs": ms(kona_cl.get("fcu_attrs")),
    "new_pay":   ms(kona_cl.get("new_pay")),
    "get_pay":   ms(kona_cl.get("get_pay")),
}

BAR = "══════════════════════════════════════════════════════════════"
print(f"\n{BAR}")
print(f"  xlayer-node  Simple Bench  |  {dur}s · {conc} workers")
print(BAR)
print()
print(f"  THROUGHPUT")
print(f"  TPS                    {m['tps']} TX/s  ({m['n_conf']} confirmed)")
print()
print(f"  TX INCLUSION LATENCY")
print(f"  p50                    {m['p50']} ms")
print(f"  p99                    {m['p99']} ms")
print()
print(f"  BLOCK PRODUCTION")
print(f"  Avg block time         {m['avg_bt']} s  ({m['n_blks']} blocks)")
print()
print(f"  ENGINE API  (kona (CL) = reth (EL) + engine-bridge overhead)")
print(f"  {'Call':<16} {'kona (CL)':>10}  {'reth (EL)':>10}  {'bridge ovhd':>11}  n")
print(f"  {'-'*16} {'-'*10}  {'-'*10}  {'-'*11}  ---")
calls = [
    ("FCU (no attrs)", "fcu"),
    ("FCU+attrs",      "fcu_attrs"),
    ("new_payload",    "new_pay"),
    ("get_payload",    "get_pay"),
]
for label, key in calls:
    tot  = kona_total.get(key)
    reth = ms(reth_el.get(key))
    n    = reth_el[key]["n"] if reth_el.get(key) else "-"
    print(f"  {label:<16} {fmt(tot):>10}  {fmt(reth):>10}  {diff(tot,reth):>11}  {n}")

print(f"\n{BAR}\n")
PYPRINT
