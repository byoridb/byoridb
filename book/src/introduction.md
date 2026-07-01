# ByoriDB

nGQL 호환 쿼리 언어와 온톨로지 추론(RDFS-Plus) 레이어를 갖춘, Rust로 작성된 그래프 데이터베이스입니다. 분산은 설계에 반영되어 있으나 다중 노드 배포는 아직 로드맵 단계이며, 현재 프로덕션은 단일 노드입니다.

## 개요

ByoriDB는 다음과 같은 특징을 갖춘 독립적인 그래프 데이터베이스 구현입니다:

- **메모리 안전성**: 가비지 컬렉션 없이 Rust의 소유권 모델 사용
- **두려움 없는 동시성**: 안전한 병렬 처리
- **무비용 추상화**: 저수준 성능을 갖춘 고수준 API
- **모던 Async/Await**: Tokio 런타임 기반

## 아키텍처

이 프로젝트는 여러 크레이트로 구성되어 있습니다:

| 크레이트 | 설명 |
|-------|-------------|
| `byoridb-common` | 핵심 데이터 타입 (Value, Vertex, Edge, DataSet) |
| `byoridb-kvstore` | KV 스토리지 계층 (redb, 순수 Rust) |
| `byoridb-codec` | 행(row) 인코딩/디코딩 |
| `byoridb-storage` | 정점과 간선을 위한 스토리지 서비스 |
| `byoridb-meta` | 메타데이터 관리 (space, 스키마) |
| `byoridb-parser` | nGQL 쿼리 언어 파서 |
| `byoridb-executor` | 쿼리 실행 엔진 |
| `byoridb` | 그래프 서비스 및 API 계층 |
| `byoridb-client` | 클라이언트 라이브러리 및 CLI |

## 주요 기능

### 지원하는 nGQL

**DDL (데이터 정의 언어)**
- `CREATE SPACE` / `DROP SPACE`
- `CREATE TAG` / `DROP TAG` / `ALTER TAG`
- `CREATE EDGE` / `DROP EDGE` / `ALTER EDGE`
- `SHOW SPACES` / `SHOW TAGS` / `SHOW EDGES`

**DML (데이터 조작 언어)**
- `INSERT VERTEX` / `UPDATE VERTEX` / `DELETE VERTEX`

**DQL (데이터 조회 언어)**
- `FETCH PROP` - 정점 속성 조회
- `GO` - 그래프 순회
- `MATCH` - Cypher 스타일 패턴 매칭
- `LOOKUP` - 인덱스 기반 쿼리
- `FIND PATH` - 최단 경로 쿼리
- `RECOMMEND` - 유사 버텍스 추천 (구조적 / 임베딩 코사인 + WHERE 필터)

**온톨로지 / 시맨틱**
- `CREATE CLASS … SUBCLASS OF … [DISJOINT WITH …]` - 클래스 계층 (TBox)
- `CREATE EDGE … TRANSITIVE/SYMMETRIC/INVERSE OF/SUBPROPERTY OF/DOMAIN/RANGE/CHAIN` - 시맨틱 관계
- RDFS-Plus forward-chaining materialization + `owl:sameAs` 동치 + `WHY` 설명
- `CREATE SHAPE … ON <class> (…)` / `CHECK SHAPE` - SHACL식 shape 검증 (required/datatype/값 술어)
- `CHECK CONSISTENCY` / `is_a(v, "class")` - 일관성 검사 / 계층 인지 쿼리

### 분산 (설계 — 다중 노드 배포는 진행 중)

> ⚠️ 아래는 라이브러리 레이어의 분산 메커니즘이며, 실행 바이너리는 현재 단일 노드만 노출합니다. 진짜 다중 노드 배포는 launcher 통합(로드맵) 후 가능합니다.

- **Raft 합의**: 리더 선출, 로그 복제, 스냅샷
- **Meta 서비스**: 스키마 관리를 위한 gRPC/HTTP 서버
- **파티셔닝**: VID 기반 consistent hashing
- **복제 팩터(Replica Factor)**: 다중 노드 복제 (설계)

### 성능 최적화
- **Bloom filter**: 약 1%의 거짓 양성률(false positive rate)
- **Block Cache**: 256MB LRU 캐시
- **배치 연산**: 다중 키 조회
- **Arena 할당**: malloc 대비 16배 개선
- **Predicate Pushdown**: 스토리지 계층에서의 필터링
- **RPC 압축**: gzip/zstd 지원

## 빠른 예제

```sql
-- Create a space
CREATE SPACE my_space(vid_type=INT64);
USE my_space;

-- Define schema
CREATE TAG person(name STRING, age INT64);

-- Insert data
INSERT VERTEX person(name, age) VALUES 1:('Alice', 30);
INSERT VERTEX person(name, age) VALUES 2:('Bob', 25);

-- Query data
FETCH PROP ON person 1;
MATCH (n:person) RETURN n;
GO FROM 1 OVER * YIELD vertex;
```

## 라이선스

이 프로젝트는 Apache 2.0 License로 라이선스됩니다.
