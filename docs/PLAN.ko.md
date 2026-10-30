# ByoriDB 개발 플랜

> [English](PLAN.md) | **한국어**
>
> 갱신: 2026-10-30. 코드 기준선: `main`의 `6d77d89` (버전 **v0.4.21**).
> 소스 기준 계획이며 실시간 배포 상태 보고가 아닙니다. 이번 개편 중의 검증 결과와
> 미출시 작업은 아래에서 별도로 구분합니다.

이 문서는 **Agent를 직접 사용자로 삼는 범용 그래프·온톨로지 DB**의 공식 개발 계획입니다.
사람은 배포를 운영·평가·통제하고 도입을 결정합니다. Agent 메모리는 엔진의 여러 사용례 중
하나입니다. [제품 철학과 디자인](PRODUCT_DESIGN.ko.md)은 목표 계약을 정의하고, 이 계획은
그 구현·수용 상태를 관리합니다. [사용자 로드맵](../book/src.ko/development/roadmap.md)은
공개 요약입니다. 양 언어를 함께 갱신합니다. 이전 근거·과거 작업 ID·운영 사고 기록은
[보관 문서](archive/PLAN.pre-readiness-2026-10-30.ko.md)에 유지합니다.

## 제품 방향

Rust property graph 코어를 유지하면서 클래스·규칙·추론 근거·설명·제약·과거 사실을
다루는 시맨틱 기능을 강화합니다. Agent가 권한 내 기능을 발견하고, 타입 있는 연산을
요청하며, 범위가 제한된 결과·근거를 검사하고, 안전하게 재시도하거나 충돌을 해결하는
흐름을 주 인터페이스로 설계합니다. 조회 정확성·복구·권한 검사는 선결 조건입니다.
호출 수·토큰·지연보다 작업 성공과 최종 상태의 정확성을 먼저 평가합니다.
Agent 효율이 경쟁 제품보다 우수하다는 사실은 아직 입증하지 않았습니다.

DB는 저장·조회·추론·설명·제약 평가·프로토콜 계약을 담당합니다. 애플리케이션과
오케스트레이션 계층은 목표·모델 선택·업무 액션·워크플로·승인·외부 write-back을
담당합니다. 프로토콜 어댑터는 같은 DB 의미를 노출하며 권한을 부여하거나 미지원 엔진
기능을 제공하지 않습니다. 별도 MCP 서버는 `byori` 소유일 수 있으며 이 저장소에 이미
구현됐다고 주장하지 않습니다. 논리적 추론 근거와 데이터 출처·감사 이력은 서로 다른
계약입니다. 설명은 기록된 사실·규칙에서의 도출을 보여주며 외부 출처의 진실성을 보증하지
않습니다. 일부 nGQL 및 RDFS/OWL 계열 기능의 구현은 nGQL·OWL·SHACL·RDF·SPARQL
전체 준수를 뜻하지 않습니다.

제품 방향은 승인됐으며 아래 검증된 구현 단위를 제외한 새 기계 계약은
**PLANNED 또는 IN PROGRESS**입니다. 다음 표는 foundations 이전 코드 기준선입니다. 제안하는 필드·도구 이름·멱등성·revision 검사·snapshot
보장을 현재 API 기능으로 공개하지 않습니다.

## 코드 기준선과 제약

| 영역 | 코드 기준선에서 제공 | 남은 제약 |
|---|---|---|
| 런타임 | Rust 1.90; 단일 서버가 graph service와 redb 저장소를 HTTP/gRPC에서 공유 | Raft·라우팅 구성요소는 존재하지만 지원되는 다중 노드 런타임·장애 수용 검증은 없음 |
| 조회 | vertex/edge CRUD, `FETCH`, `GO`, `MATCH`, tag/edge `LOOKUP`, 경로 탐색, `EXPLAIN`/`PROFILE` | 자원 가드의 완전한 결과 또는 오류 계약이 통일되지 않음; compound는 트랜잭션이 아님 |
| 인터페이스 | `read_only`를 검사하는 HTTP/gRPC 쿼리 실행; 문자열 열을 반환하는 edge `WHY` | 타입 있는 쿼리 파라미터 실행·구조화된 proof/capability envelope·제안한 Agent 쓰기 계약은 미구현 |
| 최근 수정 | node label AND/OR, edge type alternation, edge `FETCH @rank`, 잔여 입력·알 수 없는 MATCH 함수 거부 | 관련 회귀를 유지; edge FETCH의 rank 생략은 여전히 모든 rank를 조회 |
| 인덱스 | tag/edge 동등·지원되는 로컬 범위 조건, 역방향/tag-to-VID 인덱스, edge/index 원자적 갱신 batch | 지원하지 않는 범위 타입은 scan fallback; 분산 ordered range 미지원 |
| 정점 ID | INT64와 standalone `FIXED_STRING(N)` CRUD·탐색 | `WHY` 문법과 `RECOMMEND`는 정수 ID 전용; 분산 문자열 매핑 미지원; VID 타입 변경은 새 space 필요 |
| 온톨로지 | 클래스 계층·disjoint/equivalent, 시맨틱 edge 규칙, 2-link chain, 현재뷰 추론, edge `WHY`, DRed, shape | 상한 도달 시 추론이 불완전할 수 있음; 정점 타입 설명·shape/추론 정책 보강 필요; `sameAs` 병합은 되돌릴 수 없음 |
| 추천 | 구조·embedding·영속 HNSW·혼합 추천 | 문자열 VID 일관성과 실제 워크로드 수용 검증은 별도 작업 |
| 시간 | asserted current/history 원자적 기록, 단조 transaction time, vertex/edge `FETCH ... AS OF` | temporal `MATCH`/`GO`, 명시적 valid-time 구간, 이력 목록 API, retention/GC, 과거 추론 재구성 미지원 |
| 복구 | current와 history 테이블을 보존하는 전체 snapshot 백업·복원 | CLI verify가 내용 무결성·실제 복원 성공을 보장하지 않음; 증분/PITR 절차 없음 |
| 보안 | HTTP/gRPC 인증 공유, 영속 사용자·기본 role, 재귀 권한 검사, 로컬 세션 회수, 설정 가능한 로그인 제한·세션 TTL | native TLS·space 권한·클러스터 세션 회수 없음; metrics 공개 정책 명시 필요 |

저장소의 범위 조회 primitive는 Boolean·정수·지원되는 부동소수 경계를 받지만 모든
쿼리·타입 조합이 인덱스를 사용하지는 않습니다. 문자열 키 인코딩은 사전순이 아닙니다.
지원 범위 확대 시 실행기와 `EXPLAIN`을 함께 검증합니다. 현재 Compose replica는 데이터를
공유하는 클러스터가 아니라 독립 서버입니다.

## 상태와 개발 순서

**DONE**은 구현·수용 검증·문서의 근거가 갖춰진 상태입니다. **IN PROGRESS**는 미완료
브랜치 작업이며 릴리스된 기능이 아닙니다. **PLANNED**는 수용 기준 미통과,
**GATED**는 명시한 수요나 설계 결정이 필요한 상태입니다. 의존성은 고정 작업 ID로
표시합니다. 단계는 우선순위이며 확정 일정이 아닙니다. 독립적인 검증·계약 설계·문서는
병행할 수 있습니다. 합의된 설계에 대한 의존성과 구현 완료에 대한 의존성을 구분하며,
지원한다고 알리는 기능은 구현 수용 검증을 통과해야 합니다. 보안·정확성 결함은 단계와
관계없이 기능 개발보다 우선합니다.

### 릴리스 선결 작업 — 의존성 감사

| ID / 상태 | 작업 | 수용 기준 |
|---|---|---|
| DEP-1 / DONE (unreleased) | 현재 의존성 감사 실패를 별도의 호환성 검토 변경으로 해결 | 의존 경로·수정 버전을 조사하고 호환 가능한 dependency/lockfile을 갱신. 최신 audit와 workspace 검증 통과 및 자동 릴리스 경로의 audit 필수화. 남은 advisory와 릴리스 판단은 명시적으로 기록. |

2026-10-30 기준선 lockfile 감사는 `pulldown-cmark`, `rustls`, `rustls-webpki`,
`zstd`, `zstd-safe`, `zstd-sys`에서 **취약점 10건**을 보고했습니다. foundations 변경은
수정된 zstd 의존성을 사용하는 tonic 0.14.9를 포함해 호환 버전을 갱신합니다.
압축을 끄거나 권고를 무시하지 않으며 현재 후보 audit는 **취약점 0건**입니다.
Gzip·zstd·무압축 요청/응답은 실제 gRPC 왕복 테스트로 검증합니다.

릴리스 워크플로는 서명 비밀값 preflight 다음에 정확한 release ref를 감사한 뒤
빌드·발행합니다. Auto-release는 버전 변경 전과 각 push 후보에서 audit하며 rebase
이후 후보도 재검사합니다. 별도 CI의 release-build만 audit를 기다리던 공백을
해소하는 변경입니다. 통합 전까지 워크플로 수정은 로컬 상태이며, 로컬 audit만으로
GitHub Actions 실행·릴리스 성공을 입증하지 않습니다.

`bincode` 1.3.3의 unmaintained 경고는 별도 저장 형식 마이그레이션 제약(#105)으로
유지하며 취약점을 무시한 것이 아닙니다. 최종 workspace·압축·audit 로컬 수용
검증을 모두 통과했으며 아래에 기록합니다. 통합과 호스팅된 릴리스 실행은 별도입니다.

### 1단계 — 정확성과 기계 계약

| ID / 상태 | 작업 | 의존성 | 수용 기준 |
|---|---|---|---|
| QRY-1 / DONE (unreleased) | `OPTIONAL MATCH` 오류 전파 | 없음 | 정상적인 매칭 없음은 null binding 유지. optional 호출의 실행 오류와 패턴 해석 오류를 그대로 전파. 저장소 오류·일부 행 처리 후 실패·연속 optional 경로를 검증. 집중 회귀 8개와 workspace·문서 검증 통과. 아래 이전 개발 단위의 검증 기록 참조. 하위 경로가 자체적으로 삼키는 오류는 QRY-2에 남고 DEP-1은 출시 준비 요건으로 남음. |
| QRY-2 / IN PROGRESS | scan·탐색·결과 완전성 계약 통일 | QRY-1; API-2/AGT-2 결과 계약 공동 설계 | 상한 경로와 하위 MATCH 인덱스/partition의 오류 무시를 감사. cap 미만·경계·초과에서 쿼리가 정의한 완전한 결과 또는 명시적 오류 반환. 사용자 `LIMIT`은 정상 쿼리 의미. 부분 결과 모드는 별도 설계·명시적 opt-in·기계 판독 가능한 사유가 필요하며 현재 제공을 약속하지 않음. |
| QRY-3 / PLANNED | 추론 완전성과 복구 | QRY-2 오류 계약 | cap·오류가 추론 완료로 보고되지 않음. 실패 주입으로 asserted/derived 쓰기 상태를 확인하고 재시도·재구성이 중복·낡은 근거 없이 수렴. rollback 또는 재개 복구를 먼저 정의하며 오류 반환만으로 rollback을 보장하지 않음. |
| QRY-4 / PLANNED | 실제 서버 deadline·취소 | QRY-2; 쓰기는 QRY-3 | 실행 중인 무거운 쿼리를 검증된 취소 지점에서 중단하고 자원·진단 상태를 정리하며 HTTP/gRPC에서 안정적인 오류 반환. 쓰기 중단·원자성 경계 검증. 설정값이나 클라이언트 연결 종료만으로 취소를 구현했다고 보지 않음. |
| API-1 / PLANNED | 타입 있는 쿼리 파라미터 | 설계 즉시; 실행 수용은 QRY-1–2 | HTTP/gRPC로 ID·유니코드·null·list를 문자열 보간 없이 전달. 누락·타입 오류 일관성, 기존 compound 변수와 구분. |
| API-2 / IN PROGRESS | 오류·값 타입·capability 계약 | 최소 envelope 설계 즉시; 전체 수용은 QRY-1–4와 TXN-1 | 오류·재시도·결과 상태 의미, 정수 정밀도·복합값·버전 호환 정책 고정. AGT-1 구현 전에 discovery envelope 정의. runtime 검증을 통과한 capability만 제공. 쓰기 결과를 모른다는 상태를 안전한 재시도가 가능한 성공·실패로 표현하지 않음. |
| AGT-1 / IN PROGRESS | 구조화된 연산과 권한 내 schema/capability 탐색 | 합의한 API-2 discovery/오류/값 envelope; 파라미터 실행은 API-1; 기존 인증·권한 적용; space별 권한 주장은 SEC-3 이후 | read-only 탐색과 계약 fixture부터 시작. 권한 내 schema와 구현된 capability만 버전·상한·권한 맥락과 함께 반환. 없음·거부·미지원 상태, read-only 제한, schema 누출 방지 검증. 타입 있는 연산은 점진 추가하며 텍스트 쿼리도 동일한 권한·결과 규칙 적용. |
| AGT-2 / PLANNED | 범위가 제한된 결과·근거 envelope | API-2 설계, QRY-2; 해당 보장은 QRY-3/4와 TXN-1 | 쿼리 완전성·페이지 완료·추론 상태 구분. snapshot/revision 지원 또는 부재 명시. 행·바이트·근거·실행 예산 제한과 이어 읽기 일관성·출처 범위 정의. 잘림을 완전한 성공으로 바꾸지 않으며 예산 거부·미지원 모드 명시. |
| EVAL-1 / IN PROGRESS | 재현 가능한 Agent 작업 하네스 | 기준선 fixture 즉시; 구현한 capability별 버전 관리 | 탐색·관계 조회·설명·철회·timeout 후 재시도·동시 쓰기·접근 거부 평가. 비용보다 최종 상태·근거의 정확성과 거부된 변경의 부재를 먼저 검증. 모델/버전/날짜·prompt/settings·데이터/seed·반복 횟수·예산 고정, 실행 전 성공 기준 선언. 기준선의 미지원 작업을 기록하고 동등하게 성공한 작업끼리 호출·토큰·지연 비교. |

QRY-1은 `main`의 커밋 `10a7811`에 구현·검증했으며 릴리스 발행은 확인하지
않았습니다. `codex/agent-foundations`의 foundations 변경은 DEP-1, QRY-2의
MATCH 후보·가변 길이 완전성, API-2/AGT-1의 discovery v1, 결정적 EVAL-1 fixture를
진행합니다. QRY-2에는 LOOKUP/path, projection/decode 오류, 최적화 count 감사가
남습니다. Discovery가 타입 있는 query 파라미터·일반 연산 envelope·space별 권한을
구현하는 것은 아니므로 API-2와 AGT-1은 IN PROGRESS입니다. EVAL-1의
[fixture 절차](../tests/fixtures/agent-workflows/README.ko.md)는 정확한 답·거부된 부작용·
최종 상태를 검사하며 자율 모델 실행을 주장하지 않습니다.
QRY-3 실패·복구 설계, OPS-1 복원 fixture, PERF-1 기준선 수집도 전체 단계 완료를
기다리지 않고 시작할 수 있습니다. 모든 표 항목의 수용 기준을 통과해야 1단계 완료이며,
초기 계약 설계만으로 후속 쓰기·보안·취소 동작을 완료 처리하지 않습니다.

### 2단계 — 복구·신뢰할 수 있는 쓰기·접근 경계

| ID / 상태 | 작업 | 의존성 | 수용 기준 |
|---|---|---|---|
| OPS-1 / PLANNED | 백업 내용 검사·복원 시험 | 없음 | 누락·잘림·손상을 탐지. 격리된 DB에 복원해 current/history 조회, tombstone, schema, index, 사용자, 문자열 ID 매핑 비교 및 복구 시간 기록. 원본·기존 대상 보호. |
| OPS-2 / PLANNED | 저장 형식 버전·업그레이드 정책 | OPS-1 | package와 독립적인 형식 버전 기록. 이전 fixture, 미지원 버전 거부, 백업·복구와 데이터 보존 검증. 지원 migration 범위 문서화. |
| TXN-1 / PLANNED | compound의 부분 쓰기 계약 | QRY-1; API-2 쓰기 결과 공동 설계 | 실패 전 문장이 commit될 수 있음을 문서·검증에 반영. 실패 문장·부분 반영 상태와 재시도 안전성 계약을 고정하고 rollback을 암시하지 않음. |
| TXN-2 / PLANNED | 단일 space DML 원자적 batch | TXN-1, OPS-2, QRY-3 | 지원 문장·상한 정의. 실패·crash 주입 시 current, history, index, counter, 지원하는 추론·근거 효과가 전부 또는 전무. 미지원 연산은 변경 전에 거부. |
| SEC-3 / PLANNED | 영속 space별 권한 | API-2/AGT-1과 ACL·탐색 오류 공동 설계; SDK 의존 없음 | direct·compound·profile·discovery·관리 경로의 space 간 접근 차단. grant/revoke·재시작·로컬 세션 회수 검증. 조회 endpoint로 schema·근거·연산 결과가 누출되지 않음. |
| SEC-1 / PLANNED | 지원하는 TLS 배포 구성 | 없음; 즉시 착수 가능 | 신뢰할 수 있는 proxy 또는 native TLS, 비공개 backend listener, header/credential 마스킹과 인증서 교체 시험. 신뢰하지 않는 망에서 credential 평문 전송 경로 없음. |
| AGT-3 / PLANNED | 멱등성과 영속 쓰기 결과 조회 | TXN-2, OPS-2, SEC-3; 합의한 API-2 쓰기 envelope | 범위를 정한 요청 key/fingerprint·변경·영속 결과를 원자적으로 commit. 동시 중복·같은 key의 다른 payload·crash/restart·timeout 후 조회 검증. 보존·만료를 정의하고 불명·만료 결과를 안전이 확인된 쓰기로 재실행하지 않음. 조회·재반환 권한 검사와 저장 상한 적용. |
| AGT-4 / PLANNED | expected revision·충돌·preview/apply | TXN-2, AGT-3, SEC-3; 합의한 API-2 쓰기 envelope | expected revision 검사와 지원 변경을 원자 적용. preview는 요청·영향 상태·만료를 묶으며 apply 시 권한·관련 revision 재검사. 낡거나 만료된 계획은 변경 없이 거부하고 preview 자체가 권한을 부여하지 않음. 동시 쓰기·권한 회수·결정적 충돌 응답 검증. |

Agent의 공유 쓰기 전에 권한 범위를 제한해야 하므로 SEC-1/3을 앞당겼으며 SDK를
기다리지 않습니다. 설정 가능한 로그인 제한(SEC-2)은 구현됐습니다. 클러스터 세션
회수(SEC-4)는 분산과 함께 GATED입니다. SEC-1/3 통과 전에는 망 격리를 포함한
[SECURITY.md](../SECURITY.md)의 단일 노드 운영 경계를 따릅니다.

현재 저장 형식 결정인 `bincode` 1.3.3 유지([#105](https://github.com/byoridb/byoridb/issues/105))를
따릅니다. unmaintained advisory는 DEP-1의 취약점 검출과 별개입니다. 교체에는 단순 버전
갱신이 아니라 디스크·Raft·snapshot·wire migration이 필요합니다. 준비 시 `KVStoreError`의
encoder 오류 타입 노출도 분리합니다.

### 3단계 — 온톨로지 일관성과 근거

| ID / 상태 | 작업 | 의존성 | 수용 기준 |
|---|---|---|---|
| SEM-6 / PLANNED | `WHY`·추천의 문자열 VID 일관성 | QRY-1–3 | 동등한 INT64/string fixture가 외부 ID를 보존한 동등한 결과 제공. 없는 ID 조회는 매핑 생성 없음. |
| SEM-3 / PLANNED | 추론된 정점 타입 설명 | QRY-3, SEM-6; AGT-2 근거 envelope | domain/range·class 도출의 규칙·전제를 설명하고 삭제·철회·재시작 후에도 정확성 유지. 도출 근거와 외부 출처의 진실성 구분. |
| SEM-4 / PLANNED | shape/추론 정책·관계 cardinality | QRY-3, TXN-1 | asserted/inferred 및 쓰기 시/명시적 검증 모드를 정의. 관계 개수 하한·상한, 추론·철회와 조치 가능한 위반 설명 검증. SHACL 전체 준수를 암시하지 않음. |
| SEM-7 / PLANNED | `sameAs` 영향 미리보기·충돌·병합 기록 | OPS-1, QRY-3, AGT-4 | ID·관계·제약 영향 미리보기, 충돌 정책·결정적 canonical ID 검증, 병합 기록 보존 및 revision·권한 검사하에 적용. undo는 별도 보존·분리 설계가 필요하며 계속 미지원. |
| SEM-8 / PLANNED | assertion 출처와 신뢰 정책 | API-2와 설계 즉시; 전체 수용은 OPS-2, QRY-3, TXN-2 | asserted/derived 상태와 출처 이력·수집 주체·모델 생성 여부를 구분. 출처 부재는 unknown으로 유지하고 규칙 전제로 사용 가능한지를 명시적 정책으로 결정. 텍스트·구조화·bulk 쓰기에 같은 정책 적용. 기존 assertion 이관 정책을 정의하고 갱신·삭제·재시작·백업/복원에서 출처 이력 보존. 출처 기록이 진실성을 보증하지 않음. |

functional/inverse-functional property(SEM-1)와 일반 chain(SEM-2)은 구체적 사용례,
실행 상한·근거·철회 설계를 착수 조건으로 둡니다(GATED). 분산 materialization(SEM-5)은
지원되는 분산 런타임도 필요합니다.

### 4단계 — SDK·import·프로토콜 어댑터

| ID / 상태 | 작업 | 의존성 | 수용 기준 |
|---|---|---|---|
| API-3 / PLANNED | 공식 Python 또는 TypeScript SDK 하나 | API-1–2, AGT-1–2; AGT-3/4는 수용 통과 후 노출 | Agent 연동 개발자의 수요로 언어 결정. 버전 조합·세션 갱신·`429`/`Retry-After`·취소·타입 있는 결과 검증. 결과·권한 의미 보존 및 미지원 연산 거부. |
| API-4 / PLANNED | import와 일반 쓰기의 의미 일치 | QRY-3, OPS-1, SEM-4 | bulk/일반 쓰기의 검증·ID·index·counter·history·추론·shape 비교. 우회는 명시적 모드와 검증된 재구성·검사 단계 필요. |
| AGT-5 / PLANNED | 프로토콜/MCP 어댑터 일치·인증 | API-1–2, AGT-1–2, EVAL-1; 쓰기 어댑터는 AGT-3/4와 SEC-3 추가 | HTTP/gRPC·SDK·선정한 Agent 어댑터에서 공통 fixture 실행. 타입·권한·완전성·예산·근거·쓰기 결과 보존. 어댑터 소유권·버전 지원 정의. 서버는 `byori`에 있을 수 있으며 transport 제공이 엔진 지원을 뜻하지 않음. |

### 5단계 — 운영 수용 검증

| ID / 상태 | 작업 | 의존성 | 수용 기준 |
|---|---|---|---|
| SEC-5 / PLANNED | 운영 endpoint·수명주기 정책 | SEC-1, SEC-3, OPS-1–2 | metrics/진단 접근·보존 정책과 접근 검증, 실제 runtime metric 기반 dashboard/alert, 검증된 복구·업그레이드 runbook. Agent credential·schema·근거 누출 없이 운영자가 상태를 관측할 수 있음을 검증. |

### 6단계 — 정확성·Agent 평가에 기반한 성능

| ID / 상태 | 작업 | 의존성 | 수용 기준 |
|---|---|---|---|
| PERF-1 / PLANNED | 정확성을 검증하는 벤치마크 | 기준선은 즉시; 수용은 QRY-1–2와 EVAL-1에 일치 | 처리량보다 먼저 결과·실패 검증. 실제 동시 요청, 데이터·seed·장비·build·warmup·지연 분포·메모리·durability 기록. Agent 작업 성공과 엔진 속도 별도 보고. |
| PERF-2 / PLANNED | LDBC batch-read 수용([#10](https://github.com/byoridb/byoridb/issues/10)) | PERF-1 | 외부 Q9를 기존 batch API로 전환. 결과 일치 및 이슈의 규모·환경에서 `<10s` 목표 측정. 엔진 batch는 이미 구현됨. |
| PERF-3 / GATED | index·planner·병렬·streaming 최적화 | PERF-1–2, EVAL-1; 실측 병목 | baseline·통제군을 확보하고 제한된 변경 후 정확성·작업 성공·호출/토큰·지연·메모리 비교. Agent 비용 우위는 동등하게 성공한 작업끼리 검증하며 비용 절감으로 잘못된 상태·근거를 상쇄하지 않음. |

## 후속 확장과 착수 조건

| ID / 상태 | 방향 | 착수·완료 근거 |
|---|---|---|
| TMP-2 / GATED | asserted `MATCH/GO AS OF` | OPS-1–2, QRY-2. snapshot 선택 정의, 과거 인접관계·속성·삭제·역방향 edge와 현재뷰 무회귀 검증. |
| TMP-1/3/4 / GATED | valid-time 쓰기·이력 조회·retention/GC | 시간축 의미·보존 수요 정의. 백업·복구와 호출자에게 약속한 이력 보존. |
| TMP-5 / GATED | 과거 추론 | 사실뿐 아니라 당시 규칙·동일성 매핑 버전 보존. 도출 재현성을 먼저 입증. |
| RDF-1 / GATED | RDF/Turtle 연동 | API-4. IRI·blank node·datatype·다중값 매핑을 정의하고 명시한 부분집합의 왕복 보존 검증. SPARQL·전면 RDF store 재설계를 약속하지 않음. |
| DIST-1–4 / GATED | 지원되는 다중 노드 배포 | 실증된 용량·가용성 수요와 운영 담당자. bootstrap/Raft 연결·일관된 배포·권한 회수·다중 프로세스 장애/업그레이드 E2E. |
| EXEC-1 / GATED | Volcano 전환 | 중간 결과 메모리 또는 LIMIT 지연 병목 실측. [설계](VOLCANO_MIGRATION_PLAN.ko.md)를 결과 일치 검증과 함께 점진 적용. |

## 이슈·완료 기록 정리

2026-10-30 GitHub 확인 기준 열린 이슈는 **#10, #23, #24**입니다. 닫힌 이슈와 병합된
구현은 앞으로 할 개발이 아닙니다. 과거 이슈 일지를 반복하는 대신 남은 수용 범위를 기록합니다.

| 작업 | 기준선 근거 / 남은 담당 |
|---|---|
| 알 수 없는 MATCH 함수, edge UPDATE, FETCH rank, 잔여 입력 | PR #106, #107, #109, #111 병합. 회귀 유지 |
| 문자열 VID migration 문서, edge index 삭제, edge LOOKUP/type alternation | PR #112–115 병합. 이후 #129에서 bounded edge range 구현 |
| GO edge/rank 행, edge UPDATE index 원자성, 명시적 edge LOOKUP, edge range, node label AND/OR | PR #126–130 병합. 관련 이슈 닫힘 |
| [#10](https://github.com/byoridb/byoridb/issues/10) | 엔진 batch 완료. 외부 LDBC 수용 검증은 PERF-2 |
| [#23](https://github.com/byoridb/byoridb/issues/23) | 엔진 session status 계약 완료. 클라이언트 retry는 `byori` 담당([byori#54](https://github.com/byoridb/byori/issues/54)) |
| [#24](https://github.com/byoridb/byoridb/issues/24) | Linux/macOS x86_64/arm64 archive 제공. 지원 광고 전에 Windows 정책·graceful shutdown·release 검증 확정 |
| 이전 COR-1/2/3 | standalone 문자열 VID, identity 목록, license/archive packaging 완료. Windows 정책은 별도 |

## 검증과 유지 규칙

### Foundations 구현 — 2026-10-30

`codex/agent-foundations`의 로컬 변경은 다음 네 단위로 나눠 검토할 수 있습니다.

- **DEP-1:** 호환 의존성 패치와 자동 버전 갱신·릴리스 경로의 audit 필수화.
  압축은 유지하며 실제 gzip/zstd/무압축 gRPC 요청·응답의 UTF-8 값 일치를 검사합니다.
- **QRY-2 일부:** MATCH full/tag-VID/property-index 후보 scan은 cap+1을 확인하고
  scan·가변 길이 탐색 상한 초과를 오류로 반환합니다. Index partition·group-count
  오류를 전파합니다. 손상된 tag-index 인코딩은 cap 판정을 숨기거나 가짜 ID가 되지
  않고 실패합니다. 0은 무제한이며 query LIMIT/OFFSET는 scan budget을 우회하지 않습니다.
- **AGT-1/API-2 일부:** 인증을 공유하는 읽기 전용 discovery 계약 1, 제한된 스키마,
  고정 오류, HTTP/gRPC 일치. 현재 유효 역할은 전역이며 snapshot/revision과
  제안한 쓰기 계약은 미지원입니다.
- **EVAL-1 일부:** 버전이 있는 INT64/문자열 ID fixture 두 개가 새 메모리 DB에서
  discovery·정확한 답·접근 거부·최종 person 행 유지를 검사합니다. 모델 평가나
  성능 우위를 주장하지 않습니다.

압축 검증 중 네트워크·압축 처리 전의 213KB 문자열에서 lexer 스택 초과를 발견했습니다.
인용 문자열을 반복 스캐너로 읽어 해당 경로를 수정하고 escape·Unicode·위치 의미를
유지했습니다. 새 테스트는 128KiB 스택에서 긴 문자열을 처리하며, 통신 테스트도
스택을 늘리지 않고 개별 213KB 문자열·결과 4행을 검사합니다. 모든 parser 중첩·깊이의
상한을 입증한 것은 아닙니다.

MATCH 후보·탐색 회귀는 수정 전 8개 실패·정상 대조군 5개 통과를 확인했습니다.
손상 index의 cap 판정 우회도 별도로 부분 결과 성공을 재현했습니다. 최종 집중 검증은
MATCH 55개, storage 59개, discovery 15개, parser 136개, 압축 3개, EVAL fixture 2개가
통과했습니다. 전체 workspace 결과는 아래에 기록합니다.
릴리스 실패 시뮬레이션은 audit 실패가 최초 및 rebase 이후 push를 막음을 확인했습니다.
호스팅된 GitHub Actions나 릴리스 발행을 여기서 실행한 것은 아닙니다.


| 최종 로컬 검증 | 결과 |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo check --locked --workspace --all-targets --all-features` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --locked --workspace --all-features -- --test-threads=1` | PASS: 36개 suite에서 1,057개 통과, 8개 ignored. ignored 항목은 실행하지 않음 |
| `cargo build --locked --workspace --release` | PASS |
| `cargo audit --json` (로컬 cargo-audit 0.22.1) | PASS: 취약점·unsound·yanked 경고 0건, bincode unmaintained 경고 유지 |
| `mdbook build book` 및 `mdbook build book/ko` | PASS |
| 문서 일치와 로컬 링크 | PASS: 렌더링 페이지 10개, 링크·앵커 204개, 한·영 코드 예제 9쌍, 작업·상태 34행 |
| `git diff --check` | PASS |

이는 로컬 수용 검증이며 호스팅된 CI나 릴리스 결과가 아닙니다. 워크플로는 고정한
cargo-audit 0.22.2를 유지하며 로컬 audit는 0.22.1을 사용했습니다.

### Agent-first 설계 개편 — 2026-10-30

제품 설계, 공개 소개 문서, 아키텍처 요약, 기여 기준, 개발 우선순위를 한·영으로 함께
개편했습니다. 양쪽 mdBook 빌드, 로컬 파일·렌더링 링크, 한·영 작업·예제 일치,
`git diff --check`를 통과했습니다. 이번 개편은 문서와 book metadata 변경이며
제안한 Agent 계약을 구현한 것은 아닙니다. 이 문서 개편을 위해 Rust gate나
Agent·모델 평가는 다시 실행하지 않았습니다. 앞서 실행한 QRY-1 구현 검증은 아래에
보존합니다. 당시 미해결이던 DEP-1은 foundations 기록에서 갱신합니다.

### QRY-1 브랜치 검증 — 2026-10-30

최초 집중 회귀 7개는 수정 전에 5개 실패·정상 대조군 2개 통과를 확인했습니다.
수정 후 연속 OPTIONAL 회귀 1개를 더한 **8개 모두** 통과했습니다. 해당 이전 개발 단위의
검증 기록은 다음과 같습니다.

| 검증 | 결과 |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo check --locked --workspace --all-targets --all-features` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --locked --workspace --all-features -- --test-threads=1` | PASS: 1,018개 통과, 8개 ignored. ignored 항목은 실행하지 않음 |
| `cargo build --locked --workspace --release` | PASS |
| `mdbook build book` 및 `mdbook build book/ko` | PASS |
| 변경 문서의 렌더링 링크 및 EN/KO book 경로 | PASS: 로컬 링크·앵커 410개 확인, 소스 페이지 경로 23개 일치 |
| `cargo audit` | FAIL: 기존 취약점 10건을 DEP-1로 추적. QRY-1에서 dependency·lockfile 변경 없음 |

이전 QRY-1 개발 단위의 로컬 검증 기록이며 CI 통과·병합·출시 또는 새 Agent 계약의
검증을 뜻하지 않습니다.

### 작업별 완료 검증

각 작업은 가능하면 실패하는 동작·오류 주입 회귀부터 추가하고, 구현·집중 검증·양 언어
문서 갱신 후 실제 명령·결과를 기록해야 DONE으로 전환합니다. release line 병합 전의
브랜치 작업은 릴리스된 기능이 아닙니다. 기준선과 브랜치 근거를 구분해 유지합니다.

H-1–H-6 회귀, current/history 일치, 문자열 ID, index, 추론 철회를 유지합니다.
인증 변경은 중첩 권한·영속 hydration·회수·마스킹·동시성을 검증합니다. Raft 변경은
분산 E2E도 필요하며 라이브러리 테스트만으로 운영 지원을 입증하지 않습니다.

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
cargo audit
mdbook build book
mdbook build book/ko
```

변경에 맞는 검증과 저장소 필수 gate를 실행하고 도구 부재·미실행 검증을 명시합니다.
문서 재구성 후 EN/KO book 경로와 로컬 링크를 확인합니다. 보관된 테스트 수·배포 상태·
audit 결과·벤치마크 수치를 현재 근거로 재사용하지 않습니다.
