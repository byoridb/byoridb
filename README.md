<p align="center">
  <h1 align="center">ByoriDB</h1>
  <p align="center">Rust로 작성된 온톨로지 그래프 데이터베이스 — nGQL 호환 + 온톨로지 추론(RDFS-Plus) 레이어 <em>(분산은 설계에 반영, 다중 노드 배포는 로드맵)</em></p>
</p>

<p align="center">
  <a href="#빠른-시작">빠른 시작</a> •
  <a href="#기능">기능</a> •
  <a href="#최근-업데이트">최근 업데이트</a> •
  <a href="#검증--배포-내역">검증 / 배포</a> •
  <a href="#성능">성능</a> •
  <a href="#문서">문서</a> •
  <a href="#기여">기여</a>
</p>

> **⚠️ 활발히 개발 중 — 프로덕션 안정성 미보장** — ByoriDB는 아직 초기 단계의 활발히 개발 중인 프로젝트입니다. 기능·API·nGQL 문법·온디스크 포맷·동작이 릴리스 사이에 **예고 없이 바뀔 수 있고**, 검증되지 않은 엣지 케이스가 남아 있을 수 있습니다(특히 비ASCII 데이터·대량 적재·분산 모드). 실데이터 도그푸딩으로 버그를 적극적으로 찾아 고치는 단계이므로 — **중요한 데이터의 단일 저장소로 사용하지 마시고**, 충분히 검증한 뒤 사용하세요. 현재 프로덕션 배포는 `v0.2.x` 를 추적합니다.

---

## 왜 ByoriDB 인가?

- **안전성 & 속도** — Rust의 메모리 안전성과 zero-cost abstraction 기반의 C++ 급 성능
- **분산 지향 설계** — Raft 합의·consistent hashing·수평 확장을 처음부터 염두에 둔 설계 *(다중 노드 배포는 아직 진행 중 — 현재 프로덕션은 단일 노드)*
- **모던 스택** — Tokio 비동기 런타임 + redb(순수 Rust 임베디드 KV). JVM 튜닝이나 GC 정지 없음
- **nGQL 호환** — 친숙한 그래프 쿼리 언어 + 확장되는 Cypher 스타일 지원
- **온톨로지 추론** — property graph 코어 위에 얹은 시맨틱 레이어: 클래스 계층, 시맨틱 관계 타입(transitive/symmetric/inverse/…), RDFS-Plus forward-chaining materialization, `owl:sameAs` 동치 — 단순 그래프DB를 넘어 온톨로지 DB 를 지향

## 빠른 시작

### 사전 요구사항

- Rust 1.90+ (`rust-toolchain.toml` 로 고정)
- C++ 툴체인 불필요 — 순수 Rust 스토리지(redb)
- gRPC 코드 생성을 위한 `protobuf-compiler`

### 빌드 & 실행

```bash
# 빌드
cargo build --release

# 서버 실행
BYORIDB_ROOT_PASSWORD='<root-password>' \
  cargo run --release --bin byoridb-server

# CLI 접속 (다른 터미널에서)
BYORIDB_USER=root BYORIDB_PASSWORD='<root-password>' \
  cargo run -p byoridb-client --bin byoridb-cli
```

### Docker (ACR / 사전 빌드 이미지)

```bash
docker run -e BYORIDB_ROOT_PASSWORD=secret \
  -p 9669:9669 -p 19669:19669 \
  byoridbacr.azurecr.io/byoridb-server:latest
```

### 첫 그래프 만들기

```sql
-- 스페이스 생성
CREATE SPACE my_space(vid_type=INT64);
USE my_space;

-- 스키마 정의
CREATE TAG person(name STRING, age INT64, city STRING);
CREATE EDGE follows(since INT64);
CREATE TAG INDEX idx_person_name ON person(name);

-- 데이터 삽입
INSERT VERTEX person(name, age, city) VALUES 1:('Alice', 30, 'Seoul');
INSERT VERTEX person(name, age, city) VALUES 2:('Bob', 25, 'London');
INSERT EDGE follows(since) VALUES 1->2:(2020);

-- 쿼리 — nGQL 스타일
FETCH PROP ON person 1;
GO FROM 1 OVER follows YIELD $$.person.name, follows._dst;

-- 쿼리 — Cypher 스타일 MATCH
MATCH (n:person) WHERE n.person.age > 25 RETURN n.person.name, n.person.city LIMIT 10;
MATCH (a:person)-[:follows]->(b:person) RETURN a, b LIMIT 5;

-- 통계
SHOW STATS;
SHOW TAG INDEXES;
```

## 기능

### 쿼리 언어 (nGQL + Cypher 확장)

| 분류 | 구문 |
|----------|-----------|
| **DDL** | `CREATE/DROP/ALTER SPACE/TAG/EDGE`, `CREATE/DROP TAG INDEX`, `IF NOT EXISTS / IF EXISTS` |
| **DML** | `INSERT VERTEX/EDGE`, `UPDATE VERTEX` (upsert), `DELETE VERTEX/EDGE` |
| **DQL** | `FETCH PROP ON`, `GO … OVER … [REVERSELY] YIELD`, `MATCH`, `LOOKUP`, `FIND SHORTEST PATH` |
| **MATCH** | 패턴 매칭, `WHERE` (AND/OR/NOT/CONTAINS/STARTS WITH/ENDS WITH/=~), `RETURN v/e` 객체, `OPTIONAL MATCH`, `GROUP BY`, `ORDER BY … ASC/DESC`, `LIMIT/OFFSET` |
| **함수** | `id(v)`, `properties(v/e)`, `tags(v)` / `labels(v)`, `COUNT/SUM/AVG/MAX/MIN`, `LOWER/UPPER/LENGTH/CONTAINS/STARTS_WITH/ENDS_WITH` |
| **관리** | `SHOW SPACES/TAGS/EDGES/INDEXES/STATS/SESSIONS/CREATE TAG`, `EXPLAIN/PROFILE`, `REBUILD INDEX`, `BALANCE`, `GRANT/REVOKE` |
| **온톨로지** | `CREATE CLASS … SUBCLASS OF … [DISJOINT WITH …]`, `SHOW/DESCRIBE CLASS`, `CREATE EDGE … TRANSITIVE/SYMMETRIC/INVERSE OF/SUBPROPERTY OF/DOMAIN/RANGE/CHAIN`, `INSERT EDGE sameAs()`, `CHECK CONSISTENCY`, `is_a(v, "class")`, `WHY … OVER …` |
| **shape 검증** | `CREATE SHAPE <name> ON <class> (<prop> <type> [REQUIRED] \| <prop> CHECK <expr>)`, `DROP SHAPE`, `CHECK SHAPE` — required / datatype / value-predicate 제약을 write-time 거부 + 스캔 리포트 |
| **추천** | `RECOMMEND SIMILAR TO <vid> ( OVER <edges> \| BY EMBEDDING <prop> \| BLEND … ) [WHERE …] [LIMIT k]`, `CREATE VECTOR INDEX` |

### Cypher 스타일 MATCH (v0.2.x 이후)

```sql
-- 전체 태그 데이터를 담은 vertex 객체
MATCH (v:person) RETURN v LIMIT 1;
-- → {"v": {"vid": 1, "tags": [{"name": "person", "props": {"name": "Alice", "age": 30}}]}}

-- 평탄한 맵으로서의 properties
MATCH (v:person) RETURN id(v) AS vid, properties(v) AS props LIMIT 1;

-- edge 객체
MATCH (a)-[e:follows]->(b) RETURN e LIMIT 1;
-- → {"e": {"src": 1, "dst": 2, "type": "follows", "props": {"since": 2020}}}

-- 역방향 edge 패턴 (역방향 edge 인덱스로 O(in-degree))
MATCH (p:product)<-[:produces]-(c:company) RETURN p.product.name, c.company.name;

-- OPTIONAL MATCH (LEFT JOIN 의미)
MATCH (p:person)
OPTIONAL MATCH (p)-[:works_at]->(c:company)
RETURN p.person.name, c.company.name;

-- 정규식 필터
MATCH (n:person) WHERE n.person.name =~ '.*Kim' RETURN n.person.name;

-- 집계 + GROUP BY
MATCH (n:person) RETURN n.person.city, COUNT(n) AS cnt
GROUP BY n.person.city ORDER BY cnt DESC LIMIT 5;

-- 복합 문장 (compound statement)
$f = GO FROM 1 OVER follows YIELD follows._dst AS dst;
FETCH PROP ON person $f.dst;
```

### 온톨로지 / 시맨틱 레이어

property graph 코어 위에 얹은 시맨틱 레이어. 쓰기 시 함의(entailment)를 미리 계산(forward-chaining materialization)해 두므로 읽기는 추론 비용 없이 빠릅니다.

```sql
-- 클래스 계층 (TBox) — 다중 상속 + disjoint
CREATE CLASS animal(name STRING);
CREATE CLASS dog(breed STRING) SUBCLASS OF animal;
CREATE CLASS cat() SUBCLASS OF animal DISJOINT WITH dog;

-- 시맨틱 관계 타입
CREATE EDGE ancestor() TRANSITIVE;
CREATE EDGE spouse() SYMMETRIC;
CREATE EDGE parent_of() INVERSE OF child_of;
CREATE EDGE located_in() DOMAIN place RANGE region;   -- vertex 타입 추론

-- RDFS-Plus forward chaining: INSERT 시 함의된 edge/타입을 자동 도출·저장
INSERT EDGE ancestor() VALUES 1->2:(), 2->3:();
GO FROM 1 OVER ancestor;          -- 추론된 1->3 도 함께 조회됨

-- owl:sameAs 노드 동치 (write-time canonical merge)
CREATE EDGE sameAs();
INSERT EDGE sameAs() VALUES 100->200:();   -- 두 노드를 대표 노드로 병합

-- 일관성 검사 + 클래스 계층 인지 쿼리
CHECK CONSISTENCY;                              -- disjoint 위반 탐지
MATCH (n:dog) WHERE is_a(n, "animal") RETURN n; -- subclass 까지 매칭
```

지원 규칙(RDFS-Plus): `subClassOf`/`subPropertyOf` 전이, `owl:TransitiveProperty`, `owl:inverseOf`, `owl:SymmetricProperty`, `owl:propertyChainAxiom`(2-link), `domain`/`range` vertex 타입 추론, `owl:sameAs` 동치. 삭제 시에는 더 이상 도출되지 않는 추론을 제거합니다(retraction — provenance 기반 증분 DRed). `WHY <src> -> <dst> OVER <edge>` 로 추론 사실의 유도 과정을 설명할 수 있습니다.

### shape 검증 (constraint hooks)

클래스 인스턴스가 만족해야 할 property 제약을 SHACL 스타일로 선언합니다. write-time에 위반을 거부하고, `CHECK SHAPE` 로 기존 데이터의 위반을 리포트합니다.

```sql
CREATE SHAPE personShape ON person (
    email STRING REQUIRED,   -- 필수 + 타입
    age   INT,               -- 타입
    age   CHECK age >= 0      -- 값 술어
);

INSERT VERTEX person(age) VALUES 1:(-1);  -- 거부: email 누락 + age < 0
CHECK SHAPE;                              -- 위반 리포트 (vid/shape/property/constraint)
```

targetClass 의미론으로 **subclass 인스턴스도 검증**됩니다. property graph 특성상 property는 single-valued이므로 `REQUIRED`(= SHACL `minCount≥1`)·datatype·값 술어를 다루고, 관계(edge) 카디널리티는 별도 트랙입니다.

### 유사도 / 추천

```sql
-- 구조적 유사도 (공유 이웃 Jaccard)
RECOMMEND SIMILAR TO 1 OVER follows LIMIT 10;

-- 벡터 임베딩 유사도 (코사인, 대규모는 HNSW ANN 인덱스)
RECOMMEND SIMILAR TO 1 BY EMBEDDING vec LIMIT 10;

-- 하이브리드(벡터+구조) + 온톨로지 인지 필터
RECOMMEND SIMILAR TO 1 BLEND EMBEDDING vec 0.7 OVER follows 0.3
  WHERE is_a("product") LIMIT 10;
```

### 분산 (설계 — 다중 노드 배포는 진행 중)

> ⚠️ **아직 단일 노드입니다.** 아래는 라이브러리 레이어에 구현된 분산 *메커니즘*이지만,
> 실행 바이너리(`byoridb-server`)는 현재 단일 노드 실행만 노출하며 peer/cluster 설정
> 인터페이스가 없습니다. `docker-compose.yml` 의 3 컨테이너도 Raft peer 설정이 없어
> **독립된 단일 노드 3개**로 동작합니다. 진짜 다중 노드 배포는 launcher 통합(로드맵) 후 가능합니다.

- **Raft 합의** — 리더 선출, 로그 복제, 스냅샷 (라이브러리 구현)
- **Consistent Hashing** — VID 기반 파티셔닝으로 데이터 이동 최소화 (~1/N)
- **Meta 서비스** — gRPC/HTTP를 통한 중앙집중 스키마 관리
- **복제** — 설정 가능한 replica factor + 자동 파티션 할당
- **온라인 스키마 변경** — 무중단 `ALTER TAG/EDGE ADD` (지연 마이그레이션)

자세한 내용은 [로드맵](#알려진-제약--로드맵) 및 [프로젝트 플랜](docs/PLAN.md) 의 G-2 항목을 참고하세요.

### 스토리지 엔진

- redb — ACID 내구성을 내장한 순수 Rust 임베디드 KV
- Bloom 필터 (10-bit/key, ~1% FPR)
- 256MB 블록 캐시 (LRU)
- 배치 get 최적화
- 스토리지 레이어 predicate pushdown
- OOM 방지를 위한 write-buffer 한계 설정
- **역방향 edge 인덱스** — `{space}:in-edge:{dst}:…` 로 incoming 탐색을 O(in-degree)로 처리

### 보안

- 역할 기반 인증: GOD, ADMIN, DBA, USER, GUEST
- 모든 구문 유형에 대한 RBAC 적용
- 무차별 대입 보호 (5회 로그인 실패 → 5분 잠금)
- 세션 슬라이딩 윈도우 TTL
- 랜덤 세션 ID (OsRng)
- INSERT/UPDATE 시 스키마 검증

### 운영

- 압축 지원 gRPC / HTTP API (gzip/zstd)
- CLI 클라이언트 (`byoridb-cli`)
- Prometheus 메트릭 (`/metrics`)
- 구조화된 JSON 로깅 (ELK/Loki 호환)
- 시그널 처리 기반 graceful shutdown
- 백업/복원 도구 (`byoridb-backup`)
- Azure AKS 배포 스크립트 (`deploy/azure/`)
- 활성 세션 가시성을 위한 `SHOW SESSIONS`
- **관측성**: in-flight 쿼리 게이지, 실행 중 쿼리 레지스트리(`GET /api/v1/diagnostics/queries`), 쓰기 처리량 메트릭

## 최근 업데이트

최신순 주요 변경 사항입니다. 모든 변경은 `브랜치 → CI → 머지 → 자동 배포` 흐름을 거칩니다.

| 변경 | 내용 |
|------|------|
| **온톨로지: shape 검증 (constraint hooks)** | `CREATE SHAPE … ON <class>` 로 required / datatype / 값 술어(`CHECK`) 제약 선언 — INSERT/UPDATE VERTEX write-time 거부 + `CHECK SHAPE` 스캔 리포트. targetClass 의미론으로 subclass 인스턴스도 검증 |
| **온톨로지: property chain + provenance/설명/증분 retraction (O-10~O-12)** | `owl:propertyChainAxiom`(2-link) 규칙, 추론 사실의 provenance 저장 + `WHY … OVER …` 설명, 삭제 시 provenance 기반 증분 DRed retraction |
| **대량 집계 OOM 방어** | 라벨 전용 `COUNT` 스트리밍화(88M 태그 풀스캔 시 GB급 메모리 → O(1)), `max_memory_mb` 로 쿼리 결과 누적 메모리 상한 강제(초과 시 OOM 대신 오류) |
| **쿼리 정확성 수정 (4건)** | ① `WHERE id(n)==X` 바인딩 경로에서 노드 라벨 필터가 무시되던 문제 ② `FETCH PROP ON <tag>` 가 태그 소속을 검증하지 않고 다른 태그 데이터를 반환하던 문제 ③ `GO … OVER *` 에서 `type(edge)`/`dst(edge)`/`src(edge)` 등 edge 함수가 NULL 을 반환하던 문제 ④ 집계 시 암묵 `GROUP BY`(비집계 RETURN 컬럼으로 자동 그룹화) 미동작 — 실데이터 도그푸딩으로 발견·수정 |
| **대량 쓰기 안정성 (UTF-8 로깅 패닉)** | 긴 비ASCII(한글 등) 쿼리를 로깅할 때 UTF-8 문자 경계가 아닌 고정 바이트 위치에서 잘라 서버가 패닉하던 문제 수정. 대량 INSERT 중 간헐적 connection reset 의 근본 원인이었음 |
| **nGQL 문자열 escape** | 문자열 리터럴 내 `\"` / `\\` / `\n` 등 escape 시퀀스 처리 — 값에 따옴표·백슬래시가 포함된 데이터 적재 가능 |
| **온톨로지: `owl:sameAs` 동치 + 삭제 retraction (O-8/O-9)** | 노드 동치(`sameAs`)를 write-time canonical merge(대표 노드 병합)로 처리. 삭제 시 더 이상 도출되지 않는 추론을 full re-materialization 으로 제거 |
| **온톨로지: 추론 엔진 + 클래스 계층 (O-3~O-7)** | 클래스 계층(`SUBCLASS OF`), 시맨틱 관계 타입, RDFS-Plus forward-chaining materialization(transitive/symmetric/inverse/subPropertyOf + domain/range 타입 추론), `CHECK CONSISTENCY`, `is_a()` 시맨틱 쿼리 |
| **유사도 추천 (R-트랙)** | `RECOMMEND SIMILAR TO` — 구조적 Jaccard / 벡터 임베딩(HNSW ANN) / 하이브리드 BLEND + 온톨로지 인지 필터 |
| **역방향 edge 인덱스 (O-1)** | `GO … REVERSELY` 등 incoming 탐색이 전체 엣지 풀스캔 O(E) → `{space}:in-edge:{dst}:…` 인덱스 prefix scan **O(in-degree)** 로. INSERT/DELETE EDGE가 양방향 기록. LDBC Q8 `reply_of REVERSELY` 의 120초 timeout 블로커 해소 |
| **문자열 리터럴 내 `;` 처리 수정** | `"Alice; Bob"` 같은 리터럴 안의 세미콜론을 compound separator로 오인하던 버그 제거. compound 쿼리는 파서의 정식 `Statement::Compound` 경로로 위임 |
| **DROP SPACE 완전 정리** | `DROP SPACE` 가 데이터/스키마/인덱스를 모두 purge → 동일 이름 재사용 가능 (반복 벤치 차단 요소 제거) |
| **관측성 1차** | in-flight 쿼리 게이지 + 실행 중 쿼리 레지스트리 + `/api/v1/diagnostics/queries` + 쓰기 처리량 메트릭 |
| **LOOKUP 한정 속성 필터 + 인덱스 backfill 배치** | `WHERE tag.prop == …` (`PropRef`) 필터 인식, `CREATE INDEX` backfill을 청크 배치로(데이터 로드 후 인덱스 생성 timeout 해소) |
| **파서 에러 위치 표시** | `expected X, found Y at line L, column C` 형식으로 진단성 강화 |
| **SHOW STATS 빈 타입 0 표기** | 데이터 없는 tag/edge를 누락이 아닌 `0` 으로 보고 |
| **대량 INSERT 배치** | 다중 행 INSERT를 단일 KV 트랜잭션(1 fsync)으로 묶어 대량 로드 성능 대폭 개선 |
| **인덱스 DDL 키워드 태그** | `CREATE TAG INDEX … ON Tag(name)` 처럼 태그명이 키워드(`Tag`)일 때 파싱 실패하던 문제 수정 |
| **redb 전환** | RocksDB 제거 → 순수 Rust redb. C++ 툴체인 의존성 제거 |

## 검증 / 배포 내역

### 품질 게이트

- `cargo build` / `cargo build --release` (LTO)
- `cargo clippy --all-targets --all-features -- -D warnings` (경고를 에러로)
- `cargo fmt --all -- --check`
- `cargo test --workspace -- --test-threads=1` — **직렬 실행 필수** (redb 파일 락 경합 회피)
- pre-commit 훅(`git config core.hooksPath .githooks`), CRAP 복잡도 체크 CI

### LDBC SNB 검증

- **SF0.1 로드/검증 완료** — 327,588 vertices / 1,477,965 edges 로드, `validate_counts` · smoke(person 속성, is_located_in, has_interest, knows 정/역방향) · LOOKUP 속성 필터(first_name/last_name, tag.name) 통과
- 인덱스 포함 스키마 적용 약 20초 (이전 120초 timeout 해소)
- **read query 어댑터 포팅 진행 중** — LDBC Interactive 쿼리를 nGQL로 이식하며 발견되는 엔진 갭을 동일 파이프라인으로 처리 (Q8 역방향 탐색 블로커 → 역방향 edge 인덱스로 해소)

### 배포 파이프라인

- `main` push 시 **CI** 와 **Build & Deploy** 워크플로가 실행되어 Azure AKS 프로덕션으로 자동 배포됩니다.
- 배포 환경: 단일 노드(StatefulSet `replicas: 1`), `managed-csi-premium` PVC **128Gi**, VM `Standard_D2s_v5` (2 vCPU / 8 GiB), pod 메모리 limit 6Gi.

## 아키텍처

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│ byoridb-cli  │────▶│  byoridb     │────▶│ byoridb-     │
│   (CLI)      │     │  (Server)    │     │  executor    │
└──────────────┘     └──────────────┘     └──────┬───────┘
                                                 │
                    ┌────────────────────────────┼──────────────────┐
                    │                            │                  │
              ┌─────▼─────┐           ┌─────────▼─────────┐   ┌─────▼─────┐
              │ byoridb-  │           │   byoridb-        │   │ byoridb-  │
              │   meta    │           │   storage         │   │  parser   │
              │ (Schema)  │           │  (Raft+KV)        │   │  (nGQL)   │
              └─────┬─────┘           └─────────┬─────────┘   └───────────┘
                    │                           │
              ┌─────▼──────┐           ┌────────▼────────┐
              │ byoridb-   │           │   byoridb-      │
              │  kvstore   │           │   kvstore       │
              │  (redb)    │           │   (redb)        │
              └────────────┘           └─────────────────┘
```

| 크레이트 | 역할 |
|-------|------|
| `byoridb-common` | 핵심 데이터 타입 (Value, Vertex, Edge, DataSet) |
| `byoridb-kvstore` | KV 스토리지 레이어 (redb, 순수 Rust) |
| `byoridb-codec` | proto/JSON 이중 포맷 행 인코딩/디코딩 |
| `byoridb-storage` | 스토리지 서비스, Raft 합의, 인덱싱 |
| `byoridb-meta` | 메타데이터 관리, 파티션 할당 |
| `byoridb-parser` | nGQL 쿼리 언어 파서 (lexer + AST) |
| `byoridb-executor` | 쿼리 계획·실행 엔진 (MATCH, GO, LOOKUP, …) |
| `byoridb-graph` | 그래프 서비스, HTTP/gRPC 서버, 인증 |
| `byoridb-client` | 클라이언트 라이브러리 및 CLI |

## 성능

*벤치마크 환경: Apple Silicon, Rust 1.90, redb*

### 쿼리 지연 시간

| 연산 | 지연 시간 |
|-----------|---------|
| 단건 조회 (FETCH) | **143µs** |
| 100개 vertex 배치 | **172µs** |
| 1-hop 탐색 (GO) | **1.28ms** |
| 3-hop 탐색 | **3.41ms** |
| 인덱스 조회 (LOOKUP) | **2.98ms** |
| 전체 파이프라인 (parse→plan→execute) | **110µs** |

### 처리량 (부하 테스트, 단일 노드)

| 시나리오 | QPS | 에러율 |
|----------|-----|------------|
| 동시 클라이언트 50 | **31 K QPS** | 0% |
| 동시 클라이언트 100 | **12.5 K QPS** | 0% |

### BFS / Dijkstra (그래프 탐색 벤치)

| 시나리오 | 시간 | 기준 대비 |
|----------|------|--------------|
| BFS chain far / 4096 노드 | 1.70 ms | −39% |
| BFS star hub 16K 이웃 | 2.12 ms | −27% |
| Dijkstra weighted / 4096 | 2.61 ms | −7% |

### 주요 최적화

| 기법 | 효과 |
|-----------|--------|
| Arena 할당 (Bumpalo) | 할당 ~16배 빠름 |
| Bloom 필터 | 디스크 읽기 20–40% 감소 |
| 배치 get | KV 왕복 50–80% 감소 |
| Predicate pushdown | 데이터 전송 10–100배 감소 |
| RPC 압축 (zstd) | 대역폭 30–50% 절감 |
| `scan_stream` BoxStream | BFS 핫패스 −39–49% |
| 역방향 edge 인덱스 | incoming 탐색 O(E) → O(in-degree) |

### 벤치마크 실행

```bash
cargo bench -p byoridb-executor --bench graph_traversal
cargo bench -p byoridb-kvstore  --bench wal_overhead
```

## HTTP API (v0.2.x)

```
POST   /api/v1/session            → 세션 생성 (session_id 반환)
DELETE /api/v1/session/:id        → 로그아웃
POST   /api/v1/query              → nGQL 실행 (JSON 본문: {session_id, query})
POST   /api/v1/query/json         → 동일, 원시 JSON 문자열 반환
GET    /api/v1/diagnostics/queries → 실행 중인 쿼리 목록 (관측성)
GET    /health                    → 헬스 체크
GET    /metrics                   → Prometheus 메트릭
GET    /api/v1/metrics            → JSON 형태 메트릭
```

## 문서

전체 문서는 [**ByoriDB Book**](book/src/SUMMARY.md) 에서 볼 수 있습니다.

빠른 링크:

- [소개](book/src/introduction.md) — ByoriDB란?
- [빠른 시작](book/src/getting-started/quickstart.md) — 5분 안에 실행하기
- [nGQL 문법](book/src/guide/ngql-syntax.md) — 쿼리 언어 레퍼런스
- [아키텍처 개요](book/src/architecture/overview.md) — 시스템 설계
- [배포](book/src/operations/deployment.md) — 프로덕션 배포
- [프로젝트 플랜](docs/PLAN.md) — 현재 상태, 남은 작업, 의사결정 가이드

## 소스에서 빌드

```bash
# Rust 1.90 필요 (rust-toolchain.toml 로 고정)
rustup update

# 디버그 빌드
cargo build

# 릴리스 빌드 (LTO 활성화)
cargo build --release

# 전체 테스트 실행 (직렬 — redb 파일 락 경합)
cargo test --workspace -- --test-threads=1

# 디버그 로깅과 함께 실행
RUST_LOG=info cargo run --release --bin byoridb-server
```

## 알려진 제약 / 로드맵

| 항목 | 상태 |
|------|--------|
| 역방향 edge 인덱스 (incoming O(in-degree) 조회) | ✅ 구현됨 (2026-06) |
| `SHOW SESSIONS` (라이브 데이터) | ✅ v0.2.15 구현됨 |
| 다중 노드 샤딩 (파티션의 노드 간 분산) | 진행 중 (현재 단일 노드 배포) |
| 가변 길이 경로 `*1..n` 실행 (`MATCH`/`FIND ALL SHORTEST PATHS`) | ✅ 구현됨 (2026-06) |
| shape 검증 (SHACL식 required/datatype/값 술어 제약) | ✅ 구현됨 (2026-07) |
| 관계(edge) 카디널리티 제약 | 계획됨 |
| Geography WKB/WKT 디코딩 | 계획됨 |
| `RETURN *` (전체 변수) | 계획됨 |
| MVCC / 분산 2PC 트랜잭션 | 미계획 (비용 대비 효과) |
| 노드 간 TLS | 네트워크 격리로 완화; TLS 종단 프록시 권장 |
| Grafana 대시보드 템플릿 | 계획됨 |
| 중앙 로그 파이프라인 (Fluentd/Filebeat) | 계획됨 |

## 기여

기여를 환영합니다! 자유롭게 Pull Request를 제출해 주세요.

1. 저장소를 fork
2. feature 브랜치 생성 (`git checkout -b feature/amazing-feature`)
3. pre-commit 훅 활성화: `git config core.hooksPath .githooks`
4. 테스트 실행: `cargo test --workspace -- --test-threads=1`
5. 커밋: `git commit -m 'feat: add amazing feature'`
6. push 후 Pull Request 열기

## 라이선스

이 프로젝트는 [Apache 2.0 License](LICENSE) 로 라이선스됩니다.
