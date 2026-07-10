<p align="center">
  <h1 align="center">ByoriDB</h1>
  <p align="center">온톨로지 + 시간축을 갖춘 그래프 데이터베이스 — Rust · nGQL 호환 · RDFS-Plus 온톨로지 추론 <em>(bitemporal 시간축과 다중 노드 배포는 로드맵)</em></p>
</p>

<p align="center">
  <a href="#비전--발전-방향">비전</a> •
  <a href="#빠른-시작">빠른 시작</a> •
  <a href="#기능">기능</a> •
  <a href="#성능">성능</a> •
  <a href="#검증--배포">검증 / 배포</a> •
  <a href="#로드맵">로드맵</a> •
  <a href="#문서">문서</a>
</p>

> **⚠️ 활발히 개발 중 — 프로덕션 안정성 미보장.** 기능·API·nGQL 문법·온디스크 포맷이 릴리스 사이에 예고 없이 바뀔 수 있습니다. 실데이터 도그푸딩으로 버그를 찾아 고치는 단계이니 **중요한 데이터의 단일 저장소로 사용하지 마세요.** 고정 semver가 아니라 `main` HEAD 를 커밋 SHA(`sha-<short>`)로 연속 배포(CD)합니다 (현재 Cargo `0.2.4`, git 태그 `v0.2.1`).

---

## 비전 / 발전 방향

대부분의 그래프 데이터베이스는 **"지금 무엇이 연결돼 있는가"** 를 저장합니다. ByoriDB는 그 위에 두 개의 축을 더한 **온톨로지 + 시간축을 갖춘 그래프 DB** 를 지향합니다.

**1. 의미 — 온톨로지 (구현됨)**
넣은 사실에서 *논리적으로 따라 나오는 것*을 DB가 스스로 압니다. 클래스 계층 덕분에 "모든 `product` 를 달라"는 질의가 하위 유형까지 포함하고, `owl:sameAs` 로 여러 소스에 흩어진 **같은 실체를 하나로 해소(entity resolution)** 하며, 모든 추론 사실은 `WHY` 로 **왜 그렇게 도출됐는지 설명**할 수 있습니다. 이미 구현·배포돼 있습니다.

**2. 시간 — bitemporal (로드맵, 설계 단계)**

> ⚠️ 아직 구현 전입니다. 아래는 지향하는 방향이며 현재 코드에는 시간축이 없습니다.

각 TAG와 엣지의 상태가 *시간에 따라 어떻게 변해왔는지*를 1급 시민으로 다루는 것을 목표로 합니다. 사실이 현실에서 참인 기간(**valid time**)과 DB가 그것을 기록한 시점(**transaction time**)을 모두 보존하는 **bitemporal** 모델로, `AS OF <시점>` 시점 질의와 상태 변화 이력 조회를 지원하는 방향입니다. 덮어쓰기 대신 이력을 **append-only 로 누적**하고, 삭제는 유효 구간을 닫는 것으로 표현합니다. 시간축은 **단언된 사실에 먼저** 적용하며, 온톨로지 추론 레이어와는 "현재 뷰(current view)" 경계로 분리해 독립적으로 발전시킵니다.

**지향하는 정체성** — **추론(온톨로지) · 시간(bitemporal) · 근거(provenance)를 코어에 네이티브로 갖춘 범용 그래프 DB.** 오늘날 지식·기억 시스템(AI 에이전트 메모리, 회사 두뇌, 지식그래프)은 이 세 가지 — 여러 소스의 같은 실체를 하나로 해소하고, 사실이 시간에 따라 어떻게 변했는지 보존하고, 왜 그렇게 판단했는지 설명하는 능력 — 을 저장소 위 앱 레이어에서 매번 재구현합니다. ByoriDB는 그것을 엔진에서 제공해, 그런 시스템이 올라서는 **substrate**가 되는 것을 지향합니다. 특정 도메인에 묶이지 않는 범용 엔진입니다.

| 축 | 상태 |
|---|---|
| property graph 코어 (nGQL/Cypher, `MATCH`/`GO`/`LOOKUP`) | ✅ 구현·배포 |
| 온톨로지 / 시맨틱 레이어 (계층·추론·entity resolution·설명) | ✅ 구현·배포 |
| 시간축 — bitemporal 이력 + 시점 질의 (`AS OF`) | 🔬 설계 중 |
| 병렬 쿼리 실행 — 대규모 스캔·집계 코어 병렬화 | 📋 설계 완료·미구현 |
| 다중 노드 분산 | 🚧 진행 중 (현재 단일 노드) |

**핵심 특징 (현재)**

- **Rust 네이티브** — 메모리 안전 + C++급 성능. redb(순수 Rust 임베디드 KV) 기반이라 JVM·GC 정지, C++ 툴체인이 없음
- **nGQL + Cypher 확장** — 친숙한 그래프 쿼리 언어에 Cypher 스타일 `MATCH` 지원
- **온톨로지 추론** — 클래스 계층, 시맨틱 관계 타입, RDFS-Plus/OWL 2 RL forward-chaining materialization, `owl:sameAs` 동치
- **분산 지향 설계** — Raft·consistent hashing 을 처음부터 반영 *(다중 노드 배포는 진행 중, 현재는 단일 노드)*

## 빠른 시작

### 사전 요구사항

- Rust 1.90+ (`rust-toolchain.toml` 로 고정 — `rustup update` 로 맞춤)
- `protobuf-compiler` (gRPC 코드 생성). C++ 툴체인은 불필요 (순수 Rust redb)

### 빌드 & 실행

```bash
cargo build --release

# 서버 실행
BYORIDB_ROOT_PASSWORD='<root-password>' \
  cargo run --release --bin byoridb-server

# CLI 접속 (다른 터미널)
BYORIDB_USER=root BYORIDB_PASSWORD='<root-password>' \
  cargo run -p byoridb-client --bin byoridb-cli

# 전체 테스트 (직렬 필수 — redb 파일 락 경합 회피)
cargo test --workspace -- --test-threads=1
```

### Docker (사전 빌드 이미지)

```bash
docker run -e BYORIDB_ROOT_PASSWORD=secret \
  -p 9669:9669 -p 19669:19669 \
  byoridbacr.azurecr.io/byoridb-server:latest
```

### 첫 그래프 만들기

```sql
CREATE SPACE my_space(vid_type=INT64);
USE my_space;

CREATE TAG person(name STRING, age INT64, city STRING);
CREATE EDGE follows(since INT64);
CREATE TAG INDEX idx_person_name ON person(name);

INSERT VERTEX person(name, age, city) VALUES 1:('Alice', 30, 'Seoul');
INSERT VERTEX person(name, age, city) VALUES 2:('Bob', 25, 'London');
INSERT EDGE follows(since) VALUES 1->2:(2020);

-- nGQL 스타일
FETCH PROP ON person 1;
GO FROM 1 OVER follows YIELD $$.person.name, follows._dst;

-- Cypher 스타일 MATCH
MATCH (n:person) WHERE n.person.age > 25 RETURN n.person.name, n.person.city LIMIT 10;
MATCH (a:person)-[:follows]->(b:person) RETURN a, b LIMIT 5;
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
| **관리** | `SHOW SPACES/TAGS/EDGES/INDEXES/STATS/SESSIONS`, `EXPLAIN/PROFILE`, `REBUILD INDEX`, `GRANT/REVOKE` |
| **온톨로지** | `CREATE CLASS … SUBCLASS OF … [EQUIVALENT TO …] [DISJOINT WITH …]`, `CREATE EDGE … TRANSITIVE/SYMMETRIC/INVERSE OF/SUBPROPERTY OF/EQUIVALENT TO/DOMAIN/RANGE/CHAIN`, `INSERT EDGE sameAs()`, `CHECK CONSISTENCY`, `is_a(v, "class")`, `WHY … OVER …` |
| **shape 검증** | `CREATE/DROP SHAPE`, `CHECK SHAPE` — required / datatype / 값 술어 제약 |
| **추천** | `RECOMMEND SIMILAR TO <vid> ( OVER <edges> \| BY EMBEDDING <prop> \| BLEND … ) [WHERE …] [LIMIT k]`, `CREATE VECTOR INDEX` |

```sql
-- 전체 태그 데이터를 담은 vertex 객체 / 평탄한 properties
MATCH (v:person) RETURN v LIMIT 1;
MATCH (v:person) RETURN id(v) AS vid, properties(v) AS props LIMIT 1;

-- 역방향 edge 패턴 (역방향 인덱스로 O(in-degree))
MATCH (p:product)<-[:produces]-(c:company) RETURN p.product.name, c.company.name;

-- OPTIONAL MATCH (LEFT JOIN) · 정규식 · 집계
MATCH (p:person) OPTIONAL MATCH (p)-[:works_at]->(c:company) RETURN p.person.name, c.company.name;
MATCH (n:person) WHERE n.person.name =~ '.*Kim' RETURN n.person.name;
MATCH (n:person) RETURN n.person.city, COUNT(n) AS cnt GROUP BY n.person.city ORDER BY cnt DESC LIMIT 5;

-- 복합 문장 (compound statement)
$f = GO FROM 1 OVER follows YIELD follows._dst AS dst;
FETCH PROP ON person $f.dst;
```

### 온톨로지 / 시맨틱 레이어

property graph 코어 위에 얹은 시맨틱 레이어. 쓰기 시 함의(entailment)를 미리 계산(forward-chaining materialization)하므로 읽기는 추론 비용 없이 빠릅니다.

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

-- forward chaining: INSERT 시 함의된 edge/타입을 자동 도출·저장
INSERT EDGE ancestor() VALUES 1->2:(), 2->3:();
GO FROM 1 OVER ancestor;          -- 추론된 1->3 도 함께 조회됨

-- owl:sameAs 노드 동치 (write-time canonical merge)
CREATE EDGE sameAs();
INSERT EDGE sameAs() VALUES 100->200:();   -- 두 노드를 대표 노드로 병합

-- 일관성 검사 + 클래스 계층 인지 쿼리 + 설명
CHECK CONSISTENCY;                              -- disjoint 위반 탐지
MATCH (n:dog) WHERE is_a(n, "animal") RETURN n; -- subclass 까지 매칭
WHY 1 -> 3 OVER ancestor;                       -- 추론 사실의 유도 과정 설명
```

지원 규칙(RDFS-Plus + OWL 2 RL): `subClassOf`/`subPropertyOf` 전이, `owl:equivalentClass`/`owl:equivalentProperty`, `owl:TransitiveProperty`, `owl:inverseOf`, `owl:SymmetricProperty`, `owl:propertyChainAxiom`(2-link), `domain`/`range` vertex 타입 추론, `owl:sameAs` 동치. 삭제 시에는 더 이상 도출되지 않는 추론을 제거합니다(provenance 기반 증분 DRed retraction).

### shape 검증 (SHACL 스타일 constraint)

클래스 인스턴스가 만족해야 할 property 제약을 선언합니다. write-time 에 위반을 거부하고, `CHECK SHAPE` 로 기존 데이터의 위반을 리포트합니다. targetClass 의미론으로 subclass 인스턴스도 검증됩니다.

```sql
CREATE SHAPE personShape ON person (
    email STRING REQUIRED,   -- 필수 + 타입
    age   INT,               -- 타입
    age   CHECK age >= 0      -- 값 술어
);
INSERT VERTEX person(age) VALUES 1:(-1);  -- 거부: email 누락 + age < 0
CHECK SHAPE;                              -- 위반 리포트 (vid/shape/property/constraint)
```

### 유사도 / 추천

```sql
RECOMMEND SIMILAR TO 1 OVER follows LIMIT 10;                 -- 구조적 (공유 이웃 Jaccard)
RECOMMEND SIMILAR TO 1 BY EMBEDDING vec LIMIT 10;             -- 벡터 (코사인, 대규모는 HNSW ANN)
RECOMMEND SIMILAR TO 1 BLEND EMBEDDING vec 0.7 OVER follows 0.3
  WHERE is_a("product") LIMIT 10;                            -- 하이브리드 + 온톨로지 필터
```

### 스토리지 · 보안 · 운영

- **스토리지** — redb(ACID 순수 Rust KV), Bloom 필터(10-bit/key), 256MB LRU 블록 캐시, predicate pushdown, write-buffer 상한(OOM 방지), **역방향 edge 인덱스**(`{space}:in-edge:{dst}:…` → incoming 탐색 O(in-degree))
- **보안** — 역할 기반 인증(GOD/ADMIN/DBA/USER/GUEST) + 전 구문 RBAC, 무차별 대입 보호(5회 실패 → 5분 잠금), 세션 슬라이딩 TTL, 랜덤 세션 ID(OsRng), INSERT/UPDATE 스키마 검증
- **운영** — 압축 gRPC/HTTP(gzip/zstd), CLI, Prometheus `/metrics`, 구조화 JSON 로깅, graceful shutdown, 백업/복원(`byoridb-backup`), `SHOW SESSIONS`, 실행 중 쿼리 레지스트리(`/api/v1/diagnostics/queries`)

### 분산 (설계 — 다중 노드 배포는 진행 중)

> ⚠️ **아직 단일 노드입니다.** 아래는 라이브러리 레이어에 구현된 *메커니즘*이며, 실행 바이너리는 현재 단일 노드만 노출합니다(peer/cluster 설정 인터페이스 없음). `docker-compose.yml` 의 3 컨테이너도 **독립 단일 노드 3개**로 동작합니다. 진짜 다중 노드 배포는 launcher 통합(로드맵) 후 가능합니다.

Raft 합의(리더 선출·로그 복제·스냅샷), consistent hashing 파티셔닝(~1/N 이동), 중앙 Meta 스키마 관리, 설정 가능한 replica factor, 무중단 `ALTER TAG/EDGE ADD` 가 라이브러리로 구현돼 있습니다. 자세한 내용은 [프로젝트 플랜](docs/PLAN.md) 의 G-2 항목 참고.

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

| 쿼리 지연 | | 처리량 (단일 노드) | |
|---|---|---|---|
| 단건 조회 (FETCH) | 143µs | 동시 클라이언트 50 | 31K QPS (0% 에러) |
| 100 vertex 배치 | 172µs | 동시 클라이언트 100 | 12.5K QPS (0% 에러) |
| 1-hop 탐색 (GO) | 1.28ms | | |
| 3-hop 탐색 | 3.41ms | **그래프 탐색 벤치** | |
| 인덱스 조회 (LOOKUP) | 2.98ms | BFS chain / 4096 노드 | 1.70ms (−39%) |
| 파이프라인 (parse→plan→execute) | 110µs | BFS star hub 16K 이웃 | 2.12ms (−27%) |

주요 최적화: Arena 할당(Bumpalo, ~16배), Bloom 필터(디스크 읽기 −20~40%), 배치 get(왕복 −50~80%), predicate pushdown(전송 −10~100배), zstd RPC 압축(대역폭 −30~50%), `scan_stream` BoxStream(BFS 핫패스 −39~49%), 역방향 edge 인덱스(incoming O(E)→O(in-degree)).

```bash
cargo bench -p byoridb-executor --bench graph_traversal
cargo bench -p byoridb-kvstore  --bench wal_overhead
```

## HTTP API

```
POST   /api/v1/session            → 세션 생성 (session_id 반환)
DELETE /api/v1/session/:id        → 로그아웃
POST   /api/v1/query              → nGQL 실행 (JSON: {session_id, query})
POST   /api/v1/query/json         → 동일, 원시 JSON 문자열 반환
GET    /api/v1/diagnostics/queries → 실행 중인 쿼리 목록
GET    /health                    → 헬스 체크
GET    /metrics                   → Prometheus 메트릭
```

## 검증 / 배포

**품질 게이트** — `cargo build --release`(LTO), `cargo clippy --all-targets --all-features -- -D warnings`(경고=에러), `cargo fmt --all -- --check`, `cargo test --workspace -- --test-threads=1`(직렬 필수), pre-commit 훅 + CRAP 복잡도 CI.

**LDBC SNB** — SF0.1 로드/검증 완료(327,588 vertices / 1,477,965 edges, 스키마 적용 ~20초). read query 어댑터를 nGQL로 이식하며 엔진 갭을 동일 파이프라인으로 처리 중.

**배포** — `main` push 시 CI + Build & Deploy 워크플로가 Azure AKS 프로덕션으로 자동 배포. 단일 노드(StatefulSet `replicas: 1`), `managed-csi-premium` PVC 128Gi, VM `Standard_D2s_v5`(2 vCPU / 8 GiB), pod 메모리 limit 6Gi.

## 로드맵

| 항목 | 상태 |
|------|--------|
| 역방향 edge 인덱스, 가변 길이 경로 `*1..n`, shape 검증 | ✅ 구현됨 |
| 시간축 — bitemporal 이력 (`AS OF` 시점 질의, 상태 변화 추적) | 🔬 설계 중 (T-트랙) |
| 병렬 쿼리 실행 (대규모 스캔·집계 코어 병렬화) | 📋 설계 완료·미구현 (P-트랙) |
| 에이전트 접근 — MCP 서버 (기억 substrate 로 직접 연결) | 계획됨 |
| 다중 노드 샤딩 (파티션의 노드 간 분산) | 🚧 진행 중 (현재 단일 노드, G-2) |
| 관계(edge) 카디널리티 제약, `RETURN *`, Geography WKB/WKT | 계획됨 |
| Grafana 대시보드, 중앙 로그 파이프라인 | 계획됨 |
| MVCC / 분산 2PC 트랜잭션 | 미계획 (비용 대비 효과) |
| 노드 간 TLS | 네트워크 격리로 완화; 종단 프록시 권장 |

전체 현황·의사결정 가이드는 [프로젝트 플랜](docs/PLAN.md) 참고.

## 문서

전체 문서는 [**ByoriDB Book**](book/src/SUMMARY.md) 에서 볼 수 있습니다.

- [소개](book/src/introduction.md) · [빠른 시작](book/src/getting-started/quickstart.md) · [nGQL 문법](book/src/guide/ngql-syntax.md) · [아키텍처](book/src/architecture/overview.md) · [배포](book/src/operations/deployment.md) · [프로젝트 플랜](docs/PLAN.md)

## 기여

1. 저장소를 fork, feature 브랜치 생성
2. pre-commit 훅 활성화: `git config core.hooksPath .githooks`
3. 테스트: `cargo test --workspace -- --test-threads=1`
4. push 후 Pull Request

## 라이선스

[Apache 2.0 License](LICENSE).
