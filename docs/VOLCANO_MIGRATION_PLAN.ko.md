# Pull 기반 실행기 마이그레이션 계획

> [English](VOLCANO_MIGRATION_PLAN.md) | **한국어**
>
> 상태: **계획 단계, 구현 미착수**. 2026-07-29 실행기 코드 기준으로 검토했습니다.

이 문서는 현재 batch 중심 실행기를 Volcano 모델로도 불리는 조합 가능한 pull 기반
physical operator로 점진 전환하는 방안을 제안합니다. 현재 쿼리가 iterator를 사용한다는
의미는 아닙니다.

## 검토 이유

실행기는 목적별 모듈로 분리되어 있지만 많은 쿼리 경로가 중간 binding 또는 결과 row를
`Vec`에 수집합니다. 특히 `MATCH`는
`byoridb-executor/src/match_impl/match_executor.rs`에서 phase 단위 materialization을
사용합니다.

이 구조에는 반복되는 네 가지 비용이 있습니다.

1. **늦은 LIMIT 처리.** 일부 경로는 조기 종료하지만 filter, join, grouping, optional
   pattern이 큰 중간 collection을 요구할 수 있습니다.
2. **Peak memory.** 결과 메모리 guard는 안전하게 실패시키지만 유효한 대형 쿼리를
   streaming query로 바꾸지는 못합니다.
3. **중복된 physical 작업.** `MATCH`, `GO`, `LOOKUP`, recommendation, path 실행이
   scan, filter, projection, limit 로직의 일부를 각자 보유합니다.
4. **근사 operator 시간.** 현재 profile tree는 유용하지만 phase timing에 자식 작업이
   포함되어 exclusive operator self-time을 구분하기 어렵습니다.

Pull 모델에서는 상위 operator가 한 번에 한 row를 요청합니다. `Limit`은 더 이상 pull하지
않아 조기 종료할 수 있고, physical operator를 조합하며 안정적인 경계에 profiling을
붙일 수 있습니다.

## 범위 밖

- 실행기 전체를 한 번에 재작성하지 않습니다.
- nGQL 의미나 결과 순서를 암묵적으로 바꾸지 않습니다.
- 현재 query/result memory limit을 제거하지 않습니다.
- 미완성 multi-node launcher와 같은 변경에 섞지 않습니다.
- 이미 큰 `byoridb-executor/src/executor/mod.rs`에 함수를 직접 추가하지 않고
  목적별 모듈을 사용합니다.

## 후보 인터페이스

정확한 async 형태는 PoC로 결정해야 합니다. 시작 후보는 다음과 같습니다.

```rust,ignore
trait PhysicalOperator: Send {
    fn schema(&self) -> &PhysicalSchema;
    fn next(&mut self) -> OperatorFuture<'_>;
    fn explain(&self) -> OperatorInfo;
}
```

`OperatorFuture`는 boxed future, GAT associated future가 될 수 있고 `Stream`으로
대체될 수도 있습니다. PoC에서 allocation과 dynamic dispatch 비용을 측정한 뒤 선택합니다.

후보 operator:

- `FullScan`, `TagVidScan`, `IndexScan`, `RangeScan`
- `Filter`, `Project`, `Limit`
- `GetVertices`, `GetEdges`, `GetNeighbors`, `Expand`
- `HashJoin`, `Aggregate`, `TopK`
- `PathFind`
- 분산 지원 이후의 future `Exchange` operator

기존 `ProfileOp`, `PlanNode`, rendering, profile overlay type은 설계 입력이지만 수정 없이
재사용할 수 있다고 가정하지 않습니다.

## 마이그레이션 단계

각 단계는 독립 변경이며 old/new 결과 동등성 테스트를 포함합니다.

### V-0 — 측정 및 계약

- 대표 `LOOKUP`, `GO`, `MATCH` workload 확보.
- Ordering, duplicate, null, cancellation, timeout, error 계약 정의.
- 현재 경로의 peak memory와 latency 기록.
- 현재 scan 중 stream을 노출하는 것과 collection을 강제하는 API 구분.

종료 조건: benchmark fixture와 의미 동등성 helper가 저장소에 존재.

### V-1 — Operator runtime PoC

- 새 목적별 모듈에 operator interface 추가.
- Scan 하나와 `Project`, `Limit` 구현.
- 좁은 `LOOKUP` 형태 하나를 기본 비활성 feature flag 또는 내부 planner switch로 연결.

종료 조건: old/new path의 data와 error 동작이 같고, 새 path가 작은 입력에서 성능
회귀 없이 조기 종료를 입증.

### V-2 — Filtering 및 graph expansion

- `Filter`, `GetNeighbors`, `Expand` 추가.
- 지원되는 `LOOKUP`, `GO` 형태를 점진 전환.
- 모든 operator에 cancellation, timeout, scan/traversal limit, memory accounting 전달.

종료 조건: 대상 query family가 legacy path를 사용하지 않으며 보안·resource guard 유지.

### V-3 — Join 및 aggregation

- `HashJoin`, `Aggregate`, `TopK`, optional-pattern 의미론 추가.
- Single-pattern부터 `MATCH` 형태를 하나씩 이동.
- 현재 implicit grouping, ordering, limit, offset 동작 보존.

종료 조건: 각 이동 형태에 deterministic 및 randomized fixture differential test 존재.

### V-4 — Path 실행 및 profiling

- Streaming이 유용한 shortest/all-shortest path operator 통합.
- Child pull을 중복 계산하지 않는 inclusive/exclusive operator time 기록.
- `EXPLAIN`에 physical tree, `PROFILE`에 측정 row/time 표시.

종료 조건: Profile 합계가 문서화된 오차 범위 안에서 end-to-end 시간과 일치.

### V-5 — 이동된 batch path 제거

- 해당 statement 형태가 모두 physical operator를 기본 사용한 뒤에만 legacy path 삭제.
- 최소 한 release cycle 또는 동등한 soak 기간 이후 migration switch 제거.
- 과거 코드와 함께 집중 회귀 테스트를 삭제하지 않음.

### V-6 — Distributed operator, 보류

[PLAN.ko.md](PLAN.ko.md)의 multi-node runtime과 failure E2E gate가 끝난 뒤에만
`Exchange`, partition-aware aggregation, broadcast/shuffle join, distributed top-k를
검토합니다.

## 필수 게이트

각 단계는 다음을 통과해야 합니다.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
```

추가로 다음을 제공해야 합니다.

- Old/new 결과 및 오류 동등성.
- 순서를 보장하는 곳의 deterministic ordering 검사.
- `LIMIT` 조기 종료 증거.
- Cancellation과 timeout 동작.
- 처리량뿐 아니라 peak-memory 측정.
- Authorization, temporal read, H-series 정확성 테스트 무회귀.

## 시작 조건

Materialization, LIMIT latency, operator reuse가 막는 재현 가능한 workload가 최소 하나
있을 때만 V-0을 시작합니다. 그전에는 정확성, 보안, temporal 완성도, 지원되는 단일
노드 운영 경계를 우선합니다.
