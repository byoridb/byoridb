# ByoriDB

> **Rust로 작성된 semantic graph database — ontology inference, provenance, bitemporal history.**

ByoriDB는 property graph 코어 위에 시맨틱 레이어를 얹은 그래프 데이터베이스입니다.
nGQL 쿼리, write-time ontology 추론, 추론 근거(`WHY`) 설명, bitemporal history 조회를
단일 로컬 바이너리로 제공합니다.

> [!NOTE]
> Claude Code / Codex용 로컬 프로젝트 지식 그래프(agent memory) 제품을 찾는다면
> **[byoridb/byori](https://github.com/byoridb/byori)** 를 보세요. 이 저장소는 그 아래에서
> 동작하는 범용 데이터베이스 엔진입니다.

## 기능

- **Property graph + nGQL**: `MATCH`, `GO`, `FETCH`, `LOOKUP`, `FIND PATH`, DDL/DML
- **Ontology inference**: class hierarchy와 선택된 RDFS-Plus/OWL 2 RL 규칙의
  write-time materialization — transitive, symmetric, inverse, subproperty,
  equivalent property, 2-link property chain
- **Provenance**: 추론 edge의 rule/premise 근거를 `WHY`로 설명, `DELETE EDGE` 시
  provenance 기반 incremental retraction, 명시적 `sameAs` canonical merge
- **Bitemporal history (v1)**: asserted vertex/edge history 기록과
  vertex `FETCH ... AS OF <epoch-ms>` 조회
- **Similarity**: 구조(Jaccard)·embedding·hybrid recommendation
- **운영**: HTTP/gRPC API, CLI, backup/restore, Prometheus metrics
- **스토리지**: 순수 Rust(redb) — C++ 툴체인 불필요

전체 OWL 2 RL이나 완전한 temporal graph query를 지원한다는 뜻은 아닙니다. 상세 기능
범위, 제약, 로드맵은 [docs/PLAN.md](docs/PLAN.md)를 참고하세요.

## 아키텍처

storage-compute 분리 구조의 세 서비스로 구성됩니다.

- **Graph Service** (`byoridb-graph`): stateless 쿼리 엔진 — nGQL 파싱, 실행 조정
- **Meta Service** (`byoridb-meta`): space/schema/user/auth 메타데이터
- **Storage Service** (`byoridb-storage`): vertex/edge 저장, partitioning

로컬 standalone(단일 프로세스에 세 서비스)이 주 사용 경로입니다. 분산 컴포넌트는
코드베이스에 있지만 multi-node 운영 wiring은 완성되지 않았습니다.

## 빠른 시작

### 사전 빌드 바이너리

[Releases](https://github.com/byoridb/byoridb/releases)에서 macOS(Apple Silicon/Intel),
Linux x86_64용 `byoridb-server` / `byoridb-cli`를 받을 수 있습니다.

```bash
export BYORIDB_ROOT_PASSWORD='change-me'
./byoridb-server
curl -s http://127.0.0.1:19669/health
```

### 소스에서 빌드

Rust 1.90(`rust-toolchain.toml` 고정)과 `protobuf-compiler`가 필요합니다.
Linux/macOS를 지원합니다.

```bash
cargo build --release
BYORIDB_ROOT_PASSWORD='<password>' cargo run --release --bin byoridb-server
BYORIDB_USER=root BYORIDB_PASSWORD='<password>' \
  cargo run -p byoridb-client --bin byoridb-cli

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
```

HTTP API 세션·쿼리 예시는 [QUICKSTART.md](QUICKSTART.md)를 참고하세요.

## 문서

- [빠른 시작](QUICKSTART.md)
- [상세 제약, 로드맵, 이력](docs/PLAN.md)
- [기여 가이드](CONTRIBUTING.md)
- [Agent memory 제품 (byori)](https://github.com/byoridb/byori)

## 라이선스

[Apache License 2.0](LICENSE)
