# EIP-8130 Port Specification (AI-Executable)

**Audience**: an AI agent porting a feature from an upstream reference
implementation to this monorepo.

**Mission**: produce byte-compatible state roots with the upstream on the
same input. Discover the file map, the upstream version drift, and the
required adaptations yourself by reading both codebases.

---

## 1. References

| Resource | Path |
|---|---|
| Upstream reference | `/Users/xzavieryuan/workspace/reth-projects/base` (branch `eip-8130-v2`, frozen at `a33ab4d`) |
| Our root | `/Users/xzavieryuan/workspace/op-dev/optimism` |

The agent figures out everything else from these two roots.

---

## 2. Five binding principles

Every concrete decision derives from one of these. When a situation isn't
covered elsewhere in this doc, fall back here.

### P1. Code state matches reality

Comments, suppressions, and markers describe what the code IS, not what
it was during some earlier phase. Stale = lie. After porting a deferred
function, the deferral note goes. After fixing a warning's cause, the
suppression goes.

### P2. One name per concept

A single semantic identity gets a single identifier across the codebase.
No aliases, no decorative prefixes/suffixes that encode no new constraint.

### P3. Mirror upstream structure

If upstream has 2 private copies of a constant to dodge a cyclic dep,
mirror that. Don't consolidate, don't refactor, don't add abstractions
upstream lacks. The byte-alignment metric (§3) is the judge. When in
doubt: copy.

### P4. Compiler feedback is signal

Warnings flag real problems. The fix is the cause (visibility, dead code,
actual usage), never the suppression. No `#[allow(...)]`, no `let _ = x`,
no `_unused` prefixes used as escape hatches.

### P5. Understand a name before changing it

Two similar names may identify different things. Trace the domain (fork
name? schedule version? trait method? contract version?) before renaming.
Rename only within one domain.

These compose. Most violations break two or more.

---

## 3. Byte-alignment metric

The agent's primary correctness signal alongside compilation.

For each ported file, normalize trivial differences with sed and count
remaining divergent lines:

```bash
normalize() { sed -E '
  # Crate path renames specific to this codebase
  s/<upstream_crate_a>/<ours_crate_a>/g
  s/<upstream_crate_b>/<ours_crate_b>/g
  # ... etc
  # Branding renames
  s/<upstream_brand>/<ours_brand>/g
  # Hardfork name renames
  s/<upstream_fork>/<ours_fork>/g
'; }

normalize < $OURS/path > /tmp/o
normalize < $BASE/path > /tmp/b
diff /tmp/o /tmp/b | grep -c '^[<>]'   # divergent lines
```

The agent constructs the actual sed rules by surveying both codebases for
the rename patterns in play.

### 3.1 Tier targets

Every file you port belongs to one of these tiers. If a file's normalized
divergence exceeds its tier, you have an unjustified deviation — find and
remove.

| Tier | Lines | When applicable |
|---|---|---|
| 0 | 0 | Pure data carriers, policy modules, no upstream-version drift in their dependencies |
| A | <10 | Trivial path-qualification differences (e.g. an import that lives at a different module path here) |
| B | 10-30 | Documented structural differences (file split for size, our reth pin's stricter trait API) |
| C | 100+ | Known categorical drift: upstream-version API adaptation, modules unique to one side |

Tier C is allowed but each file in it must have a one-sentence note in
the PR description explaining the categorical cause.

---

## 4. General port workflow

### 4.1 Discover the port surface by symbol, not by filename

Do **not** scope the port by file naming convention (e.g. "files matching
`*eip8130*` or living under feature-named modules"). Upstream wires the
feature through cross-cutting files whose names don't mention the feature
at all — chainspec parsing, genesis types, hardfork enums, fork-tracker
maps, test harnesses, deployer config, devnet stack, RPC types.

Instead, scope by **symbol grep over the upstream tree**:

```bash
cd <upstream>
git grep -l "<UpstreamForkName>\|<upstream_snake_name>\|<upstream_const>" \
  -- '*.rs' '*.go' '*.sol' '*.toml'
```

Every hit is in the port surface. A file matching the symbol but not
matching the feature name pattern is exactly the kind of cross-cutting
wiring that filename-based discovery silently misses.

Repeat the grep with each name the feature touches — the fork name, the
tx-type byte, the constant identifying the spec, etc. Union the results.

### 4.2 Per-file symbol mapping before edit

For every file in the surface (§4.1), read its upstream version end-to-end.
Map every external symbol it references — does the symbol exist at the
same path in our tree? Different path? Different name? Doesn't exist at
all? This maps to four actions:

| Upstream symbol status here | Action |
|---|---|
| Exists at same path | Use as-is |
| Exists at different path | Adapt the import; consider re-export to flatten |
| Renamed | Apply the rename mechanically |
| Doesn't exist (upstream-version drift) | Build a §4.3 cookbook entry |

Build the file map and the rename table by surveying, not by guessing.

### 4.3 Upstream-version cookbook

When upstream uses an API that doesn't exist or has changed in our
dependency versions, the agent maintains a running cookbook of mechanical
substitutions for the duration of the port. Format each entry:

```
## <Old API> → <New API>
- Old:        <signature in upstream>
- New:        <signature in ours>
- Reason:     <which crate version bump caused this>
- Adaptation: <how to translate uses>
```

Apply each entry uniformly across every file the agent touches. Don't
ad-hoc translate the same drift twice.

### 4.4 Copy first, deviate when forced

For each ported file the default action is **byte-copy + apply the rename
table + apply cookbook substitutions**. Deviation beyond that requires
written justification (in the file or in a port-notes doc) citing one of:

1. A specific cookbook entry (§4.3)
2. A categorical structural difference (file size split, op-only vs
   upstream-only modules, our reth pin's trait API)
3. A P1–P5 principle that forces it

If you can't cite one of those three, you don't deviate.

### 4.5 Test blocks port too

`#[cfg(test)] mod tests` blocks are part of the file. Port them with the
same rules. They're how you discover whether your adaptations preserve
behavior.

---

## 5. Verification

### 5.1 Compile loop

After every meaningful edit:

```bash
cargo check --workspace 2>&1 | tail
cargo check -p <crate>           # tighter feedback during iteration
```

### 5.2 Diff loop

After each file is "done", run §3 normalize+diff. Confirm the count
matches the file's tier. Persist the per-file divergence counts somewhere
(commit message, port-notes doc) so the next porter can verify them
without re-running.

### 5.3 Suspicious-marker scan

Before declaring any file done, grep the touched directories for:

- `#[allow(dead_code)]`, `#[allow(unused_imports)]`, file-level `#![allow(...)]`
  → P4 violations
- `let _ = <var>;` patterns where `<var>` is a function parameter or local
  binding → P4 violations
- `TODO`, `FIXME`, `NOT YET PORTED`, `stub` markers
  → P1 violations unless they exist verbatim in upstream

Expected output: empty (or a finite list whose every entry exists verbatim
in the upstream file at the same logical location).

### 5.4 Tests

Run the project's test suite for the touched crates and the workspace.
The pre-port test count is the floor. Anything below the floor is a
regression. Anything not green is a regression.

---

## 6. Definition of done

All four must hold simultaneously:

```bash
# 1. Workspace compiles cleanly (no new warnings beyond pre-existing).
cargo check --workspace 2>&1 | grep -E "^error" | head
# Expected: empty.

# 2. Test suite green; count ≥ pre-port floor.
cargo test --workspace 2>&1 | tail -3
# Expected: "test result: ok. <N>+ passed; 0 failed"

# 3. Per-file diff within tier limits (§3.1).

# 4. Suspicious-marker scan empty (§5.3).
```

Before submitting the agent must answer:

1. Did all four checks pass?
2. For every divergence I introduced, can I cite which cookbook entry,
   structural difference, or principle justifies it?
3. Did I introduce any P1–P5 violation?

If (3) is yes, or (2) is no for any divergence, the work is not done.

---

## 7. Future re-alignment

Upstream may evolve after this port lands — bug fixes, version upgrades,
spec updates. To detect drift:

```bash
cd <upstream>
git fetch origin
git log <last-synced-commit>..origin/<feature-branch> --oneline
git log <last-synced-commit>..origin/<other-tracked-branch> --oneline -- <relevant paths>
```

When upstream commits arrive, re-run §3 against the new HEAD per file.
The expected effect of an upstream version upgrade matching ours: per-file
divergence drops as previously-categorical-C files move to tier B or A.

The "last-synced-commit" must be recorded somewhere stable (commit
message of the port commit, or a sync-tracking file).
