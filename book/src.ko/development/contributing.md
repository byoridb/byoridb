# 기여하기

[English](../../development/contributing.html)

ByoriDB 기여를 환영합니다. 이 페이지는 저장소가 실제로 적용하는 workflow를
요약합니다. 루트 `CONTRIBUTING.md`도 읽으세요. local checkout에 `AGENTS.md`가 있다면
AI를 활용한 변경에는 해당 repository-local agent instruction을 따르세요.

## 사전 요구사항

- Git
- `rust-toolchain.toml`이 선택하는 Rust 1.90
- gRPC code generation을 위한 `protobuf-compiler` (`protoc`)
- Linux 또는 macOS

production KV engine은 pure Rust이므로 RocksDB/C++은 필요하지 않습니다.

## Checkout 설정

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb

# 저장소 formatting hook 활성화
bash scripts/setup-hooks.sh

cargo build
```

현재 `main`에서 목적이 분명한 branch를 만들고 pull request도 `main`을 대상으로
여세요. 저장소에는 활성 `develop` branch가 없으므로 이를 전제로 workflow를 만들지
마세요.

## Workspace 구조

동작을 소유하는 crate를 선택하세요.

| 변경 | 위치 |
|---|---|
| 공통 value와 graph type | `byoridb-common` |
| 영속 KV 동작과 backup 구현 | `byoridb-kvstore` |
| vertex/edge/row encoding | `byoridb-codec` |
| storage, index, partition, Raft | `byoridb-storage` |
| metadata service | `byoridb-meta` |
| lexer, AST, grammar | `byoridb-parser` |
| query plan과 실행 | `byoridb-executor` |
| 인증, session, gRPC, HTTP | `byoridb-graph` |
| Rust client 또는 CLI | `byoridb-client` |
| offline import | `byoridb-bulkloader` |
| server 또는 backup binary | 루트 `src/` |

삭제된 `byoridb-core` crate를 다시 만들지 마세요. executor router인
`byoridb-executor/src/executor/mod.rs`는 작게 유지하고 새 실행 로직은 목적별 module에
추가하세요.

## 코드 규칙

- 함수, 변수, module은 `snake_case`, type은 `PascalCase`, constant는
  `SCREAMING_SNAKE_CASE`를 사용합니다.
- library crate에서는 `thiserror` typed error를 사용합니다. service/binary 경계에서는
  `anyhow`로 context를 추가합니다.
- production code에 `unwrap()`이나 `expect()`를 추가하지 않습니다.
- production code의 `println!`, `eprintln!`, `dbg!` 대신 structured `tracing` field를
  사용합니다.
- dependency는 먼저 루트 `[workspace.dependencies]`에 등록하고 member crate에서
  `workspace = true`로 참조합니다.
- 새 동작에는 inline unit test를 추가하고 cross-crate 경로에는 필요하면 integration
  test를 추가합니다.

library error 예시:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("space not selected")]
    NoSpace,
}
```

structured log 예시:

```rust
tracing::info!(space = %space_name, "query executed");
```

## 필수 local 검사

pull request에 기대되는 넓은 gate를 실행하세요.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
```

temporary redb database의 파일 경합을 막기 위해 integration test는 직렬로 실행해야
합니다. 작업 중에는 다음과 같이 좁은 명령을 사용할 수 있습니다.

```bash
cargo test --package byoridb-parser
cargo test --package byoridb-executor <test-name>
```

GitHub workflow도 workspace를 검사하고 format, lint, test job 통과 후 release binary를
빌드합니다.

## 위험도가 높은 영역

일부 module에는 추가 regression gate가 필요합니다.

### Query 정확성 regression

`docs/PLAN.md`의 H-series 경로를 바꾸면 multi-pattern MATCH를 포함해 H-1부터 H-6
regression coverage를 다시 실행하세요. `space_id`, `tag_id`, `edge_id`를 0으로
fallback하는 동작을 되살리지 마세요.

### Temporal storage

current/history storage, temporal DML, parser, plan 변경은 current view와 history를 모두
보존해야 합니다.

```bash
cargo test -p byoridb-kvstore --test temporal
cargo test -p byoridb-parser fetch_as_of
cargo test -p byoridb-executor fetch_as_of
```

이후 전체 serial workspace suite를 실행하세요. 변경 범위에 따라 vertex/edge `AS OF`,
tombstone, 같은 millisecond 쓰기, atomic current/history apply를 확인합니다.

### Raft와 distributed 코드

`byoridb-storage/src/raft/`를 바꾸기 전에 custom Raft state machine, log, snapshot,
membership, network driver를 이해해야 합니다. 다음을 실행하세요.

```bash
cargo test --workspace --all-features --test distributed_e2e_test -- --test-threads=1
```

component test 통과가 미완성 cluster launcher를 production-ready deployment로 만들지는
않습니다. 이 경계를 문서에 분명히 표시하세요.

### 인증과 권한

user, role, session, protocol 변경 시 HTTP와 gRPC 동작, durable-user cache reconciliation,
session invalidation, compound statement, `PROFILE` 권한을 모두 검사하세요. 자격 증명이나
raw session ID를 log, metric, diagnostics, error response에 포함하지 마세요.

## 문서 변경

`book/src/`의 영문 페이지가 canonical입니다. `book/src.ko/`의 한국어 페이지도
유지하고 명령, 기능 상태, 제약을 의미상 동일하게 맞추세요. 재현 가능한 환경과 결과
artifact 없이 측정 성능 수치를 게시하지 마세요.

mdBook이 설치된 경우 저장소의 문서 workflow/configuration으로 두 언어 tree를 모두
빌드하세요.

## Pull request

pull request는 한 목적에 집중하고 다음을 포함하세요.

- 문제와 선택한 동작
- security, compatibility, data migration 영향
- 추가한 test와 실행한 정확한 명령
- 사용자에게 보이는 동작의 문서 변경
- 해당하는 관련 issue link

`.env` 파일, credential, 생성된 database 파일, machine-specific build output을 commit하지
마세요. pull-request template과 CI 결과가 제출 checklist의 source of truth입니다.
