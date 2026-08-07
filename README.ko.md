# ByoriDB

[English (기본)](README.md) | [한국어](README.ko.md)

<p align="center">
  <img src="book/src/assets/byoridb-icon.png" alt="ByoriDB official icon" width="256">
</p>

> Rust로 작성된 시맨틱 그래프 데이터베이스로, 온톨로지 추론, provenance,
> 시점 이력 조회를 제공합니다.

ByoriDB는 property graph 코어와 semantic layer를 결합합니다. 하나의 standalone
서버 프로세스에서 nGQL 스타일 쿼리, write-time ontology materialization, 추론
edge 설명, 과거 시점 조회를 제공합니다.

> [!NOTE]
> Claude Code, Codex 등 코딩 에이전트용 로컬 지식 그래프 및 agent-memory 제품을
> 찾는다면 **[byoridb/byori](https://github.com/byoridb/byori)**를 참고하세요. 이
> 저장소는 그 아래에서 사용하는 범용 데이터베이스 엔진입니다.

> [!CAUTION]
> standalone 단일 노드 launcher가 현재의 주 지원 경로입니다. 저장소에는 분산
> 컴포넌트가 있지만 Storage/Raft peer bootstrap과 다중 노드 운영 wiring은 아직
> 완성되지 않았습니다. 현재 cluster mode를 production-ready로 간주하지 마세요.

## 기능

- **Property graph와 nGQL 스타일 쿼리:** `MATCH`, `GO`, `FETCH`, `LOOKUP`,
  `FIND PATH`, schema operation, data mutation.
- **Ontology inference:** class hierarchy 및 transitive, symmetric, inverse,
  subproperty, equivalent-property, 2-link property-chain을 포함한 일부
  RDFS-Plus/OWL 2 RL 스타일 규칙을 write 시점에 materialize합니다.
- **Provenance:** `WHY`는 inferred edge의 rule과 premise를 설명하며, asserted
  edge 삭제 시 provenance를 이용한 incremental retraction을 지원합니다.
- **Identity merge:** 명시적 `sameAs` edge가 되돌릴 수 없는 canonical merge를
  수행합니다. 이는 완전한 OWL semantics보다 의도적으로 좁은 범위입니다.
- **Temporal history:** asserted vertex/edge write는 current view 변경과 history
  append를 원자적으로 반영합니다. `FETCH PROP ... AS OF <epoch-ms>`로 vertex나
  edge를 특정 시점 기준으로 조회할 수 있습니다.
- **Similarity:** structural Jaccard, embedding 및 hybrid recommendation.
- **운영:** HTTP/gRPC API, 대화형 CLI, backup/restore, readiness check,
  Prometheus metrics.
- **Pure-Rust storage:** embedded KV layer는 redb를 사용하므로 C++ toolchain이
  필요하지 않습니다.

### Batch read 동작과 제한

`FETCH PROP ON <tag> vid, ...`는 current-view vertex key 전체를 한 번의 storage
`batch_get`으로 읽습니다. 결과는 입력 VID 순서를 유지하고 존재하지 않는 VID는
생략합니다. HTTP query text 제한은 1 MiB이므로 일반적인 숫자 VID 500~1,000개 요청은
transport 제한보다 충분히 작습니다. 응답 크기는 property payload에 따라 달라지며 설정된
query result-memory 제한을 적용받습니다.

`GO ... YIELD $$.tag.prop`과 `YIELD vertex`는 destination VID를 중복 제거한 뒤
projection 전에 한 번의 batch로 읽습니다. tag, property 또는 destination vertex가 없으면
`NULL`을 반환합니다. `EXPLAIN`과 `PROFILE`에는 이 작업이 `batch destination
projection` detail을 가진 `GetVertices`로 표시됩니다.

ByoriDB는 전체 OWL 2 RL rule set이나 범용 temporal query language를 구현하지
않습니다. 현재 temporal model에서는 valid time과 transaction time을 서버가 함께
생성하며 temporal `MATCH`, `GO`, `BETWEEN`, 사용자가 지정하는 `VALID FROM/TO`는
지원하지 않습니다. Inference는 current view를 사용하며 과거 inferred fact를
재구성하지 않습니다. 상세 상태와 제약은 [구현 계획](docs/PLAN.ko.md)을
참고하세요.

## 아키텍처

코드베이스는 세 service component를 중심으로 구성됩니다.

- **Graph service** (`byoridb-graph`): 쿼리를 parse, authorize, plan하고 실행을
  조정합니다. 인증 및 session state는 서버 실행 중 해당 프로세스에 존재합니다.
- **Meta service** (`byoridb-meta`): 부분적으로 구현된 cluster 경로에서 사용하는
  space, schema, partition 및 관련 metadata service를 포함합니다.
- **Storage service** (`byoridb-storage`): vertex/edge를 저장하고 partitioning과
  custom Raft 구현을 포함합니다.

지원되는 standalone launcher는 한 프로세스에서 embedded storage와 Graph HTTP/gRPC
listener를 시작합니다. Meta gRPC server는 `cluster.peers`를 설정했을 때만 시작하며,
이 설정만으로 미완성 multi-node 경로가 production-ready 상태가 되지는 않습니다.
사용자 record는 영속적이지만 live session은 프로세스 사이에 공유되지 않습니다.
서버를 재시작하면 해당 session은 무효화되며, 현재 multi-instance 배포는 session
폐기를 서로 조정하지 않습니다.

## 빠른 시작

### 릴리스 아카이브

현재 최신 공개 릴리스는
[v0.3.3](https://github.com/byoridb/byoridb/releases/tag/v0.3.3)이며 Linux x86_64,
macOS Intel 및 Apple Silicon용 archive를 제공합니다. 이 tag는 현재 `main`의 인증
강화, HTTP/gRPC session state 공유, temporal v1.1 변경 및 edge `AS OF` 조회보다
이전입니다. 아래 문서의 동작이 필요하면 `main`의 특정 commit을 고정해 source로
빌드하고, 배포 artifact는 tag 또는 commit SHA로 식별하세요.

ARM Linux, Windows, macOS signing 및 archive license 파일의 현재 제약은
[설치 문서](book/src.ko/getting-started/installation.md)를 참고하세요.

### 사전 요구사항

- Linux 또는 macOS
- `rust-toolchain.toml`에 고정된 Rust 1.90
- gRPC code generation을 위한 `protobuf-compiler`

### 빌드 및 실행

현재 standalone 서버는 `BYORIDB_ROOT_PASSWORD`가 비어 있거나 설정되지 않으면
시작을 거부합니다. 배포 환경에서는 secret manager를, 개발 환경에서는 강력한
로컬 secret을 사용하세요.

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --locked --workspace --release

export BYORIDB_ROOT_PASSWORD='replace-with-a-strong-secret'
cargo run --locked --release --bin byoridb-server
```

기본 listener는 gRPC `0.0.0.0:9669`, HTTP `0.0.0.0:19669`입니다.

```bash
curl --fail http://127.0.0.1:19669/health
curl --fail http://127.0.0.1:19669/ready
```

다른 terminal에서 CLI를 실행합니다.

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='replace-with-a-strong-secret'
cargo run --locked -p byoridb-client --bin byoridb-cli
```

CLI에는 기본 credential이 없습니다. 사용자와 비밀번호를 위 환경 변수 또는
command-line flag로 모두 제공해야 합니다.

SQL 예제와 HTTP session 흐름은 [빠른 시작 가이드](QUICKSTART.ko.md)를
참고하세요.

## 개발 검증

임시 redb database의 file/lock 경합을 피하기 위해 integration test는 반드시
직렬로 실행합니다.

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
```

CI는 commit된 `Cargo.lock`을 RustSec advisory 기준으로도 검사합니다.

Pull Request를 열기 전에 [CONTRIBUTING.ko.md](CONTRIBUTING.ko.md)를 확인하세요.

## 보안 및 배포 경계

ByoriDB는 현재 native TLS termination과 범용 login rate limiter를 제공하지
않습니다. HTTP와 gRPC를 신뢰할 수 있는 TLS termination 뒤에 두고 network access를
제한하며, listener를 외부에 노출하기 전에 rate limiting을 추가하세요.

기본 role은 권한을 `*`(모든 space)에 적용하고 현재 query language에는
space-scoped `GRANT` 문법이 없습니다. 따라서 tenant isolation 경계로 사용할 수
없습니다. Session ID는 bearer credential로 취급하고 log에 남기지 마세요.

지원하는 security model, 배포 checklist 및 비공개 취약점 제보 방법은
[SECURITY.ko.md](SECURITY.ko.md)를 참고하세요.

## 문서

- [빠른 시작](QUICKSTART.ko.md)
- [사용자 및 운영 가이드](book/src.ko/SUMMARY.md)
- [구현 상태, 제약 및 roadmap](docs/PLAN.ko.md)
- [기여하기](CONTRIBUTING.ko.md)
- [보안 정책](SECURITY.ko.md)
- [오픈소스 고지(영문)](NOTICES.md)
- [영문 문서 index](README.md)

## 라이선스

ByoriDB는 [Apache License 2.0](LICENSE)에 따라 제공됩니다.
