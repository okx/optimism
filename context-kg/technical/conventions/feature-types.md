---
name: "feature-types"
description: "Base patterns, pagination/import/export/async patterns from reusable components"
---
# Feature Types and Base Patterns

## Retry Loop Pattern
- **Base**: retry.Do[T] / Do0 / Do2
- **Description**: Generic retry wrapper with configurable Strategy (Exponential/Fixed); context-aware; ErrFailedPermanently on max attempts
- [Rule] Must check ctx.Err() at loop start; return immediately if context cancelled before sleeping

## Strategy Pattern
- **Base**: Strategy interface (Duration method)
- **Description**: ExponentialStrategy with Min/Max/MaxJitter; Fixed strategy; pluggable backoff algorithms

## Decorator Factory Pattern
- **Base**: metrics.With(registry) returning Factory
- **Description**: Wraps promauto.Factory to auto-document metrics during creation

## Option Pattern (Functional)
- **Base**: RegisterOption func(cfg *RegisterConfig)
- **Description**: Used in event.Register(); WithEmitPriority, WithEmitLimiter configure via function composition

## Event-Driven System Pattern
- **Base**: System interface with Register/Unregister/AddTracer/RemoveTracer
- **Description**: Pub-sub with named derivers; tracer observation; priority-based scheduling; atomic abort on CriticalErrorEvent

## Lifecycle Management Pattern
- **Base**: Lifecycle interface Start/Stop with context
- **Description**: Two-context model (Start context expires, Stop context can be forced); signal handling via cliapp.LifecycleCmd

## Request Batching Pattern
- **Base**: IterativeBatchCall[K, V] with Fetch until io.EOF
- **Description**: Generic batching with lazy init; supports retry; BatchElementCreator and CallResult for type-safe unpacking

## Lock-Free Tracking
- **Base**: atomic.Uint64 for counters, atomic.Bool for abort
- **Description**: Used in event.Sys for derivContext, emitContext; high-performance counters without mutex

## Generic Type Wrapper Pattern
- **Base**: RWMap[K, V], RWValue[E], Watch[E], Limiter[E]
- **Description**: Constraint-based generics; sync.RWMutex composed; minimal allocation

## RPC Client Wrapper Pattern
- **Base**: SignerClient with health check, TLS management
- **Description**: Minimal stateful wrapper; auto-closes; certman for cert rotation

## Clock Injection Pattern
- **Base**: now func() time.Time parameter in constructors
- **Description**: Enables deterministic testing; avoids global time.Now()

## Error Chain Pattern
- **Base**: errors.Join(), Unwrap(), Is() on custom error types
- **Description**: Preserves error context; supports error inspection; context.Cause() integration
