# Pull-based execution migration plan

> **English** | [한국어](VOLCANO_MIGRATION_PLAN.ko.md)
>
> Status: **planned; implementation has not started**. Last reviewed against the
> executor on 2026-07-29.

This document proposes a gradual move from the current batch-oriented executor
to composable pull-based physical operators, commonly called the Volcano model.
It is a design direction, not a promise that current queries use iterators.

## Why consider it

The executor is split across purpose-specific modules, but many query paths
still collect intermediate bindings or result rows into `Vec` values. `MATCH`
in particular contains phase-sized materialization in
`byoridb-executor/src/match_impl/match_executor.rs`.

That architecture has four recurring costs:

1. **Late LIMIT handling.** Some paths can stop early, but filters, joins,
   grouping, or optional patterns can require large intermediate collections.
2. **Peak memory.** Result-memory guards fail safely, but they do not turn an
   otherwise valid large query into a streaming query.
3. **Duplicated physical work.** `MATCH`, `GO`, `LOOKUP`, recommendation, and
   path execution each own parts of scan, filter, projection, and limit logic.
4. **Approximate operator timing.** The current profile tree is useful, but
   phase timings often include child work instead of measuring exclusive
   operator self-time.

A pull model lets an upstream operator request one row at a time. A `Limit`
operator can stop pulling, physical operators can be composed, and profiling
can be attached to stable operator boundaries.

## Non-goals

- Do not rewrite the whole executor in one change.
- Do not change nGQL semantics or result ordering implicitly.
- Do not remove current query/result memory limits.
- Do not mix this work with the unfinished multi-node launcher.
- Do not add new functions directly to the already-large
  `byoridb-executor/src/executor/mod.rs`; use purpose-specific modules.

## Candidate interface

The exact async shape requires a proof of concept. A starting point is:

```rust,ignore
trait PhysicalOperator: Send {
    fn schema(&self) -> &PhysicalSchema;
    fn next(&mut self) -> OperatorFuture<'_>;
    fn explain(&self) -> OperatorInfo;
}
```

`OperatorFuture` may be a boxed future, a GAT-based associated future, or be
replaced by a `Stream` interface. The proof of concept must measure allocation
and dynamic-dispatch cost before choosing.

Candidate operators:

- `FullScan`, `TagVidScan`, `IndexScan`, and `RangeScan`
- `Filter`, `Project`, and `Limit`
- `GetVertices`, `GetEdges`, `GetNeighbors`, and `Expand`
- `HashJoin`, `Aggregate`, and `TopK`
- `PathFind`
- future `Exchange` operators, only after distribution is supported

The existing `ProfileOp`, `PlanNode`, rendering, and profile overlay types are
inputs to the design; they are not assumed to be reusable without change.

## Migration stages

Every stage is an independent change with old/new result-equivalence tests.

### V-0 — measurement and contracts

- Capture representative `LOOKUP`, `GO`, and `MATCH` workloads.
- Define ordering, duplicate, null, cancellation, timeout, and error contracts.
- Record peak memory and latency for the current paths.
- Identify which current scans already expose streams and which APIs force
  collection.

Exit: benchmark fixtures and semantic equivalence helpers are in the repository.

### V-1 — operator runtime proof of concept

- Add the operator interface in a new purpose-specific module.
- Implement one scan plus `Project` and `Limit`.
- Route one narrow `LOOKUP` shape through the new path behind a non-default
  feature flag or internal planner switch.

Exit: old and new paths return identical data and error behavior; the new path
demonstrates early termination without a performance regression on small input.

### V-2 — filtering and graph expansion

- Add `Filter`, `GetNeighbors`, and `Expand`.
- Migrate supported `LOOKUP` and `GO` shapes gradually.
- Propagate cancellation, timeout, scan limits, traversal limits, and memory
  accounting through every operator.

Exit: focused query families no longer use the legacy path and retain all
security and resource guards.

### V-3 — joins and aggregation

- Add `HashJoin`, `Aggregate`, `TopK`, and optional-pattern semantics.
- Move `MATCH` shapes one at a time, starting with single-pattern queries.
- Preserve current implicit grouping, ordering, limit, and offset behavior.

Exit: each migrated shape has differential tests over deterministic and
randomized fixtures.

### V-4 — path execution and profiling

- Integrate shortest/all-shortest path operators where streaming is useful.
- Track inclusive and exclusive operator time without double counting child
  pulls.
- Render the physical tree in `EXPLAIN` and measured rows/time in `PROFILE`.

Exit: profile totals reconcile with end-to-end time within a documented margin.

### V-5 — retire migrated batch paths

- Remove a legacy path only after all its statement shapes use physical
  operators by default.
- Remove the migration switch after at least one release cycle or equivalent
  soak period.
- Keep focused regression tests rather than deleting them with old code.

### V-6 — distributed operators, deferred

Consider `Exchange`, partition-aware aggregation, broadcast/shuffle join, and
distributed top-k only after the multi-node runtime and failure E2E gates in
[PLAN.md](PLAN.md) are complete.

## Required gates

Each stage must pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
```

It must also provide:

- old/new result and error equivalence;
- deterministic ordering checks where ordering is promised;
- early-stop evidence for `LIMIT`;
- cancellation and timeout behavior;
- peak-memory measurements, not only throughput;
- no regression in authorization, temporal reads, or H-series correctness tests.

## Start criteria

Begin V-0 only when at least one reproducible workload is blocked by
materialization, LIMIT latency, or operator reuse. Until then, correctness,
security, temporal completeness, and the supported single-node operational
boundary take priority.
