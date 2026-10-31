# 로드맵

[English](../../development/roadmap.html) | **한국어**

2026-10-31, 출시된 **v0.4.22**(`main`의 `6851936`) 기준으로 갱신했습니다. ByoriDB는 **Agent를 직접 사용자로
삼는 범용 그래프·온톨로지 DB**를 지향합니다. 사람은 배포를 운영·평가·통제하고 도입을
결정합니다. Agent 메모리는 여러 사용례 중 하나입니다. 확인 가능한 근거와 신뢰할 수 있는
상태 변경을 통해 작업을 정확하게 끝내는 것이 목표이며, Agent 비용·속도 우위는 앞으로
측정해야 합니다.

[제품 철학과 디자인](https://github.com/byoridb/byoridb/blob/main/docs/PRODUCT_DESIGN.ko.md)은
목표 계약을 정의하고, [docs/PLAN.ko.md](https://github.com/byoridb/byoridb/blob/main/docs/PLAN.ko.md)가
구현 상태의 기준입니다. 승인된 방향·수용 계획이며 확정 릴리스 일정이 아닙니다.
읽기 전용 discovery 계약 1과 foundations 정확성 수정은 출시됐습니다. 더 넓은 Agent
계약은 PLANNED 또는 IN PROGRESS이며 미완료 작업도 플랜에 기록한 일부 구현이
출시됐을 수 있습니다.

[CI](https://github.com/byoridb/byoridb/actions/runs/41054788495)와
[Auto-release](https://github.com/byoridb/byoridb/actions/runs/41054789050)가 성공했습니다.
[v0.4.22 릴리스](https://github.com/byoridb/byoridb/releases/tag/v0.4.22)에 Linux/macOS
x86_64/arm64 `.tar.gz` 산출물 4개가 있습니다. 이전 v0.4.21/pre-foundations 상태는
과거 기록이며 아래 진행 중인 QRY-2 브랜치 변경은 미출시입니다.

## v0.4.22에서 제공하는 기능

- vertex/edge CRUD, `FETCH`, `GO`, `MATCH`, tag/edge `LOOKUP`, 경로 탐색,
  `EXPLAIN`·`PROFILE`. edge UPDATE와 지원되는 로컬 edge index range가 구현됐고,
  node label은 AND/OR를 지원합니다.
- 그래프 CRUD·탐색의 INT64 및 standalone 문자열 VID, tag/edge/역방향 index,
  영속 HNSW 추천 index.
- class hierarchy·disjoint/equivalent, semantic edge 규칙, 2-link chain,
  현재뷰 추론, edge `WHY`, DRed 철회와 shape.
- 구조·embedding·혼합 추천.
- asserted current/history 원자적 쓰기, vertex/edge `FETCH ... AS OF`, 두 테이블을
  보존하는 전체 snapshot 백업·복원.
- HTTP/gRPC 인증을 공유하는 단일 노드 redb 서버, 영속 사용자·기본 role·로컬 세션 회수,
  설정 가능한 로그인 제한·세션 TTL.
- HTTP/gRPC의 인증된 읽기 전용 discovery 계약 1: 크기가 제한된 schema, 전역 역할
  권한, 명시적인 capability와 고정 discovery 오류.
- `OPTIONAL MATCH` 오류 전파와 MATCH 후보·가변 길이 상한의 명시적 오류.
  Index partition 실패를 전파하고 손상된 tag-index key를 거부합니다.

해당 릴리스 기준선의 기능들에는 제약이 있습니다. `WHY`·추천은 문자열 VID 일관성을 보강해야 하고,
`sameAs` 병합은 되돌릴 수 없습니다. compound는 트랜잭션이 아닙니다. scan·추론 상한의
완전한 결과 또는 오류 계약이 통일되지 않았습니다. 백업 metadata 검사는 전체 내용 검사나
실제 복원 시험이 아닙니다. native TLS·space별 권한·지원되는 다중 노드 런타임도 없습니다.
일부 nGQL/OWL 계열 기능의 구현이 언어·표준 전체 준수를 의미하지 않습니다.

## 다음 개발 단계

| 순서 | 목표 | 수용 기준의 중심 |
|---|---|---|
| 릴리스 요건 — 완료 | DEP-1 DONE, v0.4.22 출시 | 수용 시 취약점 0건, bincode unmaintained 경고 1건 유지. 자동 버전 갱신·릴리스 경로에 audit 필수화, 이후 릴리스도 재통과 필요 |
| 1 | 정확성과 기계 계약(QRY-1–4, API-1–2, AGT-1–2, EVAL-1) | 완전한 결과 또는 명시적 오류, 추론 복구·실제 취소. 타입 있는 파라미터·오류, 권한 내 탐색, 범위가 제한된 결과·근거, 초기 Agent 평가 기준선 |
| 2 | 복구·신뢰할 수 있는 쓰기·접근 경계(OPS-1–2, TXN-1–2, SEC-3/1, AGT-3–4) | 복원 시험·저장 형식 버전. compound 결과 명시 후 DML 원자성. space 권한·TLS 조기 제공, 영속 멱등 결과·revision·preview/apply |
| 3 | 온톨로지 일관성과 근거(SEM-6/3/4/7/8) | 문자열 VID, 정점 타입 설명, shape/추론·cardinality 정책, revision을 검사하는 `sameAs` preview·병합 기록, assertion 출처·명시적 전제 신뢰 정책 |
| 4 | SDK·import·어댑터(API-3–4, AGT-5) | 공식 SDK 하나, bulk import 의미 일치, 프로토콜/MCP 어댑터 공통 수용 fixture와 소유권 명시 |
| 5 | 운영 수용 검증(SEC-5) | metrics/진단 접근·보존 정책, runtime 상태 관측, 검증된 복구·업그레이드 runbook |
| 6 | 정확성·Agent 평가에 기반한 성능(PERF-1–3) | 처리량·비용보다 결과 정확성·Agent 작업 성공 우선. 실제 동시 실행, LDBC batch-read 수용, 실측에 따른 최적화 |

**QRY-1은 DONE이며 v0.4.22로 출시됐습니다.** 정상적인 매칭 없음의 null binding을
유지하면서 `OPTIONAL MATCH` 실행·패턴 해석 오류를 전파하며 집중 회귀 8개를
통과했습니다. 릴리스 근거는 위 링크에 있습니다. 같은 릴리스에 MATCH 후보·가변 길이
상한의 오류, index partition 오류 전파, 엄격한 tag-index decode, 긴 문자열의 반복
스캐너도 포함됩니다. 작은 query `LIMIT`이 후보 예산을 우회하지는 않습니다.
더 넓은 QRY-2 수용은 **IN PROGRESS**입니다.

현재 소스에는 LOOKUP/FIND/MATCH 완전성, 분산 index 이어 읽기·원격 fetch 검증,
추론 복구, 단일 문장 DML 준비, 백업 무결성·복원과 서버 lifecycle 보강이 반영됐습니다.
정확한 보장과 실행 근거는
[standalone 인수 기록](https://github.com/byoridb/byoridb/blob/main/docs/DEPLOYMENT_READINESS.ko.md)을
참고하세요. QRY-2는 IN PROGRESS입니다. QRY-3, OPS-1, SEC-1은 소스 인수 기준으로
DONE이며 릴리스 게시 대기 상태입니다. 로컬 gate와 실제 복구·TLS 검증을 통과했으며
최종 commit의 호스팅 workflow와 archive를 확인해야 게시 완료가 됩니다. [TLS 배포 프로필](../operations/secure-deployment.md)이 지원 네트워크 경계입니다.
단일 문장 transaction은 향후 TXN-2의 다중 문장·추론 전체 계약을 완성하지 않습니다.
Typed operation, idempotency, revision은 계획에 남으며 실제 게시 완료는 릴리스
workflow 성공 후에만 확인합니다.

## 기계 계약의 경계

현재 HTTP/gRPC 쿼리 실행은 `read_only`를 지원하고 edge `WHY`는 문자열 열을
반환합니다. 타입 있는 쿼리 파라미터, 구조화된 proof envelope, 영속 멱등성,
revision 검사와 preview/apply는 계획입니다. 별도 MCP 서버는 `byori`에 있을 수 있으며
어댑터가 엔진 기능을 만들어내지는 않습니다. 어댑터는 권한·타입·완전성·예산·결과 상태를
보존해야 합니다.

제안한 부분 결과 모드는 미구현이며 명시적 opt-in·기계 판독 가능한 사유가 필요합니다.
페이지 완료·쿼리 완전성·추론 상태를 구분하고 snapshot 지원 여부를 정확히 밝혀야 합니다.
쓰기 오류는 rollback의 증거가 아닙니다. 멱등성에는 변경·범위를 정한 요청 key·영속 결과의
원자적 commit이 필요하며 불명·만료 결과가 무조건 재시도를 허용하지 않습니다.
preview는 권한을 부여하지 않습니다. apply는 권한·revision을 다시 검사하고 낡거나 만료된
계획을 변경 없이 거부합니다. 설명은 기록된 사실·규칙에서의 도출을 보여주며 출처 자체의
진실성을 보증하지 않습니다.

## 후속 확장에는 착수 조건을 둡니다

- **과거 그래프 조회:** 복구·snapshot 의미 확정 후 asserted `MATCH/GO AS OF`부터
  시작합니다. 과거 추론에는 규칙·동일성 매핑 버전도 필요합니다. valid-time 쓰기와
  retention은 별도 계약이 필요합니다.
- **RDF/Turtle 연동:** IRI·blank node·datatype·다중값 매핑의 지원 범위를 정의하고
  왕복 보존을 검증합니다. SPARQL이나 전면 RDF store 재설계 약속은 아닙니다.
- **분산:** 실제 용량·가용성 수요와 운영 담당자를 전제로 합니다. bootstrap, Raft/storage
  연결, 일관된 배포, 권한, 다중 프로세스 장애·업그레이드 시험을 통과한 뒤 지원을 알립니다.
- **Volcano·병렬 실행:** 중간 결과 메모리·LIMIT 지연·처리량 병목을 실측하고,
  전환 과정 내내 결과 일치를 검증합니다.

## 기존 이슈의 남은 범위

[#10](https://github.com/byoridb/byoridb/issues/10)의 엔진 batch는 구현됐으며 외부 LDBC
하네스 수용 검증이 남았습니다. [#23](https://github.com/byoridb/byoridb/issues/23)의 엔진
작업은 완료됐고 클라이언트 retry는 `byori` 담당입니다.
[#24](https://github.com/byoridb/byoridb/issues/24)는 남은 Windows 지원 정책·구현에
관한 이슈입니다. Linux/macOS x86_64/arm64 릴리스 산출물은 이미 제공합니다.

상세 계획에서 범위가 명확한 작업을 선택하고 [기여 가이드](contributing.md)를 따릅니다.
동작 수용 테스트와 영문·한국어 문서를 함께 갱신합니다. 과거 설계·사고 기록은
`docs/archive/`에 보존하며 실시간 배포 상태를 뜻하지 않습니다.
