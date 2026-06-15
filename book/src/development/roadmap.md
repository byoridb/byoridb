# 로드맵

ByoriDB의 현재 상태와 향후 계획입니다.

## 현재 버전 (v0.1)

### 핵심 기능

- [x] nGQL 파서
  - [x] DDL 문 (CREATE/DROP SPACE/TAG/EDGE)
  - [x] DML 문 (INSERT/UPDATE/DELETE VERTEX/EDGE)
  - [x] DQL 문 (FETCH, GO, MATCH, LOOKUP, FIND PATH)
  - [x] ALTER TAG/EDGE ADD (온라인 스키마 변경)

- [x] 스토리지 엔진
  - [x] Pure-Rust KV (redb) 통합
  - [x] Vertex/Edge 인코딩
  - [x] 스키마 버전 지원
  - [x] Bloom filter
  - [x] Block cache

- [x] 분산 시스템
  - [x] Raft 합의(consensus)
  - [x] 리더 선출(Leader election)
  - [x] 로그 복제(Log replication)
  - [x] 스냅샷

- [x] Meta Service
  - [x] Space 관리
  - [x] 스키마 관리
  - [x] 스키마 버저닝 (lazy migration)
  - [x] 사용자 인증

## 예정 (v0.2)

### 쿼리 개선

- [ ] 서브쿼리
- [ ] 공통 테이블 표현식 (WITH)
- [ ] 윈도우 함수
- [ ] 전문 검색(Full-text search)

### 스키마 연산

- [ ] ALTER TAG/EDGE DROP column
- [ ] ALTER TAG/EDGE MODIFY column
- [ ] 온라인 인덱스 생성

### 성능

- [ ] 쿼리 플랜 캐싱
- [ ] 병렬 쿼리 실행
- [ ] 벡터화 실행(Vectorized execution)
- [ ] 비용 기반 옵티마이저(Cost-based optimizer)

### 운영

- [ ] 온라인 백업
- [ ] 특정 시점 복구(Point-in-time recovery)
- [ ] 클러스터 리밸런싱

## 향후 (v0.3+)

### 고급 기능

- [ ] 그래프 알고리즘 (PageRank, 최단 경로 등)
- [ ] 시간성 그래프(Temporal graphs)
- [ ] 지리공간(Geospatial) 지원
- [ ] 그래프 신경망(Graph neural network) 통합

### 엔터프라이즈 기능

- [ ] 멀티테넌시(Multi-tenancy)
- [ ] 역할 기반 접근 제어(Role-based access control)
- [ ] 감사 로깅(Audit logging)
- [ ] 저장 데이터 암호화(Encryption at rest)

### 에코시스템

- [ ] Python 클라이언트
- [ ] Java 클라이언트
- [ ] JavaScript 클라이언트
- [ ] JDBC 드라이버
- [ ] Spark 커넥터

### 클라우드 네이티브

- [ ] Kubernetes operator
- [ ] Helm charts
- [ ] 오토스케일링(Auto-scaling)
- [ ] 다중 리전 복제(Multi-region replication)

## 기여하기

기능 개발에 기여하고 싶으신가요? [기여 가이드](./contributing.md)를 확인하세요.

## 기능 요청

아이디어가 있으신가요? `enhancement` 라벨을 붙여 이슈를 열어 주세요.
