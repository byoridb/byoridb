# ByoriDB

추론(온톨로지) · 시간(bitemporal) · 근거(provenance)를 코어에 네이티브로 갖춘
**실험적 temporal ontology graph 데이터베이스** (Rust).

> ⚠️ **실험 단계.** 로컬 단일 노드로만 동작합니다. 프로덕션/클라우드 배포는 운영하지
> 않으며(관련 CD 파이프라인 비활성), 기능·API·nGQL 문법·온디스크 포맷이 예고 없이
> 바뀔 수 있습니다. 중요한 데이터의 단일 저장소로 쓰지 마세요.

## 무엇인가

property graph 코어 위에 두 축을 얹은 범용 그래프 엔진입니다.

- **온톨로지 / 추론** — 클래스 계층, 시맨틱 관계 타입(transitive/symmetric/inverse/…),
  RDFS-Plus·OWL 2 RL forward-chaining materialization, `owl:sameAs` 엔티티 해소,
  provenance(`WHY`) 설명.
- **시간 (bitemporal, v1)** — vertex/edge 변경을 별도 이력에 기록하고
  `FETCH PROP ON <tag> <vid> AS OF <epoch-ms>`로 과거 시점을 읽습니다. 현재 뷰는 무회귀.

지향점: 지식·기억 시스템(에이전트 메모리 등)이 앱 레이어에서 재구현하는 것을 엔진에서
제공하는 **substrate**. 특정 도메인에 묶이지 않는 범용 엔진입니다.

## 빌드 & 실행

```bash
# 사전: Rust 1.90 (rust-toolchain.toml 고정), protobuf-compiler

cargo build --release

# 로컬 서버 실행
BYORIDB_ROOT_PASSWORD='<password>' cargo run --release --bin byoridb-server

# CLI
BYORIDB_USER=root BYORIDB_PASSWORD='<password>' cargo run -p byoridb-client --bin byoridb-cli

# 테스트 (직렬 — redb 파일 락 경합 회피)
cargo test --workspace -- --test-threads=1
```

## nGQL (요약)

`CREATE/DROP SPACE/TAG/EDGE`, `CREATE TAG INDEX`, `INSERT/UPDATE/DELETE VERTEX/EDGE`,
`FETCH`, `GO`, `MATCH`(Cypher 스타일), `LOOKUP`, `FIND PATH`,
온톨로지(`CREATE CLASS … SUBCLASS OF`, `sameAs`, `WHY`, `is_a`), shape 검증,
`RECOMMEND SIMILAR TO`, 그리고 시간축 `… AS OF <ts>`.

## 상태 / 계획

- 로컬 단일 노드. 분산(다중 노드)·클라우드 배포는 비활성(설계만 존재, `docs/PLAN.md` G-2).
- 현재 상태·로드맵·의사결정 가이드는 [`docs/PLAN.md`](docs/PLAN.md).

## 라이선스

[Apache 2.0](LICENSE)
