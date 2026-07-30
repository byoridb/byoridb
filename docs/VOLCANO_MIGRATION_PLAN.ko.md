# Volcano(iterator) 실행기 마이그레이션 플랜 (트랙 B)

> [English](VOLCANO_MIGRATION_PLAN.md) | **한국어**
>
> **현재 상태:** 계획만 있고 구현은 미착수다. 2026-07-30 `main`의 `8209f28`
> 실행기를 기준으로 다시 확인했다. 아래 최초 설계와 단계별 상세는 역사적 맥락으로
> 그대로 보존한다.

작성: 2026-06-04. 상태: **계획만 (미착수)**.

이 문서는 EXPLAIN/PROFILE 작업(트랙 A, 완료)에서 드러난 **명령형 실행기의 본질적
한계**를 해소하기 위한 단계적 재작성 계획이다. 트랙 A는 "계측 완성"으로 현재
아키텍처가 줄 수 있는 최대치를 뽑았고, 트랙 B는 그 천장을 올린다.

---

## 왜 필요한가 (트랙 A의 남은 한계)

현재 실행기(`byoridb-executor/src/executor/`, `match_impl/`)는 각 단계를 통째로
`Vec`로 materialize하는 **명령형 batch 모델**이다. PROFILE 계측을 모든 연산자에
심었지만 다음은 구조적으로 불가능하다:

1. **연산자별 wall-clock의 정확한 분리** — 부모 연산자 시간이 자식 시간을 포함
   (예: Aggregate ⊇ Scan, PathFind ⊇ GetNeighbors). 인터리빙되는 pull 모델이
   아니라 phase가 순차 materialize되므로, "이 연산자만의 순수 시간"을 못 잰다.
2. **스트리밍/조기종료 부재** — LIMIT가 있어도 candidate 전체를 모은 뒤 truncate.
   `match_executor`가 `row_limit`로 부분 완화하지만 WHERE/멀티패턴이면 무력화.
3. **연산자 재사용 불가** — MATCH/GO/LOOKUP이 각자 스캔·필터·프로젝션 로직을
   중복 구현. `executor.rs`가 god module(3,980 LoC)로 비대해진 근본 원인.
4. **메모리** — 중간 결과 전체를 힙에 올림. 대량 결과에서 OOM 위험(S-5에서 scan
   상한으로 완화했을 뿐).

Volcano(pull-based iterator) 모델은 각 연산자가 `next()`로 한 행씩 끌어오므로
(1) 연산자별 시간이 자연 분리되고 (2) LIMIT가 위에서 pull을 멈추면 아래가
자동 조기종료하며 (3) 연산자가 조합 가능한 단위가 된다.

---

## 설계 개요

```rust
// 핵심 트레잇 (async, 스트리밍)
#[async_trait]
trait PhysicalOperator: Send {
    /// 다음 행. None이면 소진. 내부에서 자신의 시간/행수를 ProfileCollector에 누적.
    async fn next(&mut self) -> Result<Option<Row>>;
    fn schema(&self) -> &Schema;
    /// EXPLAIN/PROFILE 트리 노드 메타.
    fn explain(&self) -> OperatorInfo;
}
```

- **연산자 카탈로그**: `Scan`(IndexScan/TagVidScan/FullScan), `GetNeighbors`,
  `Expand`, `Filter`, `Project`, `Aggregate`, `HashJoin`, `Limit`, `PathFind`,
  `GetVertices/Edges`. 트랙 A의 `ProfileOp`가 이미 이 분류를 예고함 — 그대로 승계.
- **시간 계측**: 각 연산자 `next()` 진입/이탈에 타이머를 걸고 **자식 pull 시간을
  차감**해 "self time"을 계산. → 트랙 A가 못 한 순수 per-operator 시간 확보.
- **플래너**: `ExecutionPlan`(논리) → `Box<dyn PhysicalOperator>`(물리) 변환기.
  현재 `explain::build_plan_tree`의 트리 구성 로직이 그대로 물리 플랜 빌더의
  골격이 된다(이미 트랙 A에서 작성됨 — 재활용).
- **EXPLAIN/PROFILE**: 물리 연산자 트리를 그대로 순회해 렌더. 트랙 A의
  `explain::render`/overlay는 거의 그대로 재사용(트리 소스만 논리→물리로 교체).

---

## 단계 (각 단계는 독립 PR, 기존 동작 유지하며 점진 전환)

**B-0 [선결] executor.rs god module 분해 준비**
`/cah:brownfield-migrate`로 모듈 분리 계획 수립. CLAUDE.md가 executor.rs 직접
확장을 금지하므로, 신규 연산자 코드는 `executor/operators/` 신설 디렉토리에.

**B-1 트레잇 + 2개 leaf 연산자 PoC**
`PhysicalOperator` 트레잇 + `Scan`(FullScan/TagVid/Index) + `Project`만 구현.
LOOKUP 한 경로를 물리 연산자로 재구현해 기존 결과와 **동등성 테스트**(같은 쿼리
→ 같은 DataSet). 나머지 쿼리는 기존 경로 유지(feature flag `volcano`).

**B-2 Filter / Limit / GetNeighbors / Expand**
LOOKUP·GO를 물리 연산자로 완전 전환. LIMIT 스트리밍 조기종료 검증
(현재 `row_limit` 휴리스틱 제거 가능). 벤치로 회귀 없음 확인
(`graph_traversal.rs` 기준선 대비).

**B-3 HashJoin / Aggregate / PathFind**
MATCH(멀티패턴·OPTIONAL→Join, GROUP BY→Aggregate), FIND를 전환. 멀티패턴이
cross join full scan하던 한계(H-6 미해결 항목)도 HashJoin으로 자연 해소 가능.

**B-4 per-operator self-time 계측 정식화**
자식 차감 타이머로 순수 시간 산출. 트랙 A의 "부모⊇자식" 근사를 정확값으로 대체.
PROFILE 출력에 `self_time` 컬럼 추가.

**B-5 레거시 경로 제거 + feature flag 회수**
모든 쿼리가 물리 연산자 경유 확인 후 `executor.rs`의 명령형 실행 함수 삭제.
god module 대폭 축소.

**B-6 (먼 미래) 분산 연산자**
`Exchange`(shuffle/broadcast) 연산자로 PLAN.md E 섹션(분산 JOIN/집계/정렬) 흡수.
단, 분산 모드(G-2) 선결.

---

## 위험 / 보류 조건

- **고위험**: 동작 중인 쿼리 엔진 전면 재작성. 각 단계마다 **기존 경로 동등성
  테스트**(같은 쿼리 → 같은 결과)를 게이트로. feature flag로 점진 전환, 회귀 시
  즉시 롤백 가능하게.
- **비용**: B-1~B-5는 큰 작업. 운영 부하/실사용자가 없는 현 시점에선 PLAN.md
  의사결정 가이드상 **온톨로지(O 트랙)보다 후순위**. 트랙 A로 관측성 요구는
  이미 충족됐으므로, B는 "성능/메모리 병목이 측정으로 확인될 때" 또는 "O 트랙이
  변길이 경로·추론에서 연산자 조합을 요구할 때" 착수 권장.
- **executor.rs 함정**: CLAUDE.md 명시 — 직접 확장 금지. B는 분해의 좋은 계기지만
  무계획 확장은 금물. B-0 선결.

---

## 트랙 A에서 이미 확보한 재사용 자산

- `profile::ProfileOp` — 물리 연산자 분류와 1:1.
- `explain::PlanNode` / `render` / `overlay_profile` — 물리 트리 렌더에 재사용.
- `explain::build_plan_tree` — 물리 플래너의 트리 구성 골격.
- 각 실행 경로의 계측 지점 — 물리 연산자 경계와 거의 일치(이전 작업이 경계를
  식별해 둠).

즉, 트랙 A는 트랙 B의 **사전 정지작업** 역할도 한다.
