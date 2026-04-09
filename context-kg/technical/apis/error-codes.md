---
name: "error-codes"
description: "Error code registry for JSON-RPC and domain-specific errors"
---
# Error Codes

## Engine API Error Codes

| Range | Category | Description |
|-------|----------|-------------|
| -38000 to -38100 | Engine errors | IsEngineError() check |
| -32600 to -32700 | Generic RPC | IsGenericRPCError() check |

## Supervisor Error Codes

| Code | Error | Description |
|------|-------|-------------|
| -320400 | ErrUninitialized | Chain not initialized |
| -320500 | ErrSkipped | Data skipped |
| -320501 | ErrUnknownChain | Unknown chain ID |
| -320600 | ErrConflict | State conflict |
| -320601 | ErrIneffective | Operation has no effect |
| -320900 | ErrOutOfOrder | Out-of-order operation |
| -320901 | ErrAwaitReplacementBlock | Waiting for replacement block |
| -321000 | ErrStop | Stop signal |
| -321100 | ErrOutOfScope | Operation out of scope |
| -321200 | ErrPreviousToFirst | Before first known block |
| -321401 | ErrFuture | Future data requested |
| -321500 | ErrNotExact | Inexact match |
| -321501 | ErrDataCorruption | Data corruption detected |

## Build API Error Codes

| Code | Error | Description |
|------|-------|-------------|
| -40100 | BuildErrCodeTemporary | Temporary build error |
| -40101 | BuildErrCodePrestate | Prestate error |
| -40110 | BuildErrCodeInvalidInput | Invalid input |
| -40120 | BuildErrCodeUnknownPayload | Unknown payload |
| -40199 | BuildErrCodeOther | Other build error |

## Stream Polling Error Codes

| Code | Error | Description |
|------|-------|-------------|
| -39001 | OutOfEventsErrCode | No more events available (backoff signal) |

## Registration Rules

[Rule] Supervisor errors must use errorCodeMap in supervisor/types/error.go for code mapping
[Rule] Engine errors checked via ErrorCode.IsEngineError() method
[Rule] Generic RPC errors checked via ErrorCode.IsGenericRPCError() method
[Rule] MaybeAsNotFoundErr normalizes block-not-found errors across RPC implementations
