# 기여하기

[English](../../development/contributing.html) | **한국어**

ByoriDB 기여를 환영합니다. 상세 정책은 repository root의
[CONTRIBUTING.ko.md](https://github.com/byoridb/byoridb/blob/main/CONTRIBUTING.ko.md)를
먼저 읽으세요. 보안 취약점은 public issue로 공개하지 말고
[SECURITY.ko.md](https://github.com/byoridb/byoridb/blob/main/SECURITY.ko.md)의 private
reporting 절차를 따르세요.

## 개발 환경

- Linux 또는 macOS
- `rust-toolchain.toml`이 선택하는 Rust 1.90
- gRPC code generation용 `protobuf-compiler`(`protoc`)

production KV engine은 pure-Rust redb이므로 RocksDB/C++는 필요하지 않습니다.

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
bash scripts/setup-hooks.sh
cargo build --locked --workspace
```

현재 `main`에서 focused branch를 만들고 PR도 `main`을 대상으로 여세요. active
`develop` branch는 없습니다.

## 코드 위치

| 변경 | 위치 |
|---|---|
| 공통 value와 graph type | `byoridb-common` |
| persistent KV와 backup | `byoridb-kvstore` |
| vertex/edge/row codec | `byoridb-codec` |
| storage, index, partition, Raft | `byoridb-storage` |
| metadata service | `byoridb-meta` |
| lexer, AST, grammar | `byoridb-parser` |
| query plan과 execution | `byoridb-executor` |
| auth, session, gRPC, HTTP | `byoridb-graph` |
| Rust client와 CLI | `byoridb-client` |
| offline import | `byoridb-bulkloader` |
| server와 backup binary | root `src/` |

삭제된 `byoridb-core` crate를 다시 만들지 마세요. 큰 실행 로직은
`byoridb-executor/src/executor/mod.rs`에 직접 쌓지 말고 목적별 module에 두세요.

## 코드 규칙

- Rust naming convention과 `rustfmt`를 따릅니다.
- library error는 typed `thiserror`, service/binary boundary의 context는 `anyhow`를
  사용합니다.
- production path에 새 `unwrap()`/`expect()`를 추가하지 않습니다.
- production logging은 `println!` 대신 structured `tracing` field를 사용합니다.
- 공통 dependency는 root `[workspace.dependencies]`에 등록한 뒤 member에서
  `workspace = true`로 참조합니다.
- 새 동작에는 focused unit test와 필요한 cross-crate integration test를 추가합니다.

## 필수 검사

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
```

temporary redb file lock 경합을 피하려고 전체 integration suite는 serial로 실행합니다.
개발 중에는 package test를 좁혀 실행할 수 있지만 마지막 gate는 full workspace입니다.

```bash
cargo test --locked --package byoridb-parser
cargo test --locked --package byoridb-executor <test-name>
```

temporal storage를 바꾸면 current view와 history, same-millisecond write, tombstone,
vertex/edge `AS OF`, atomic application을 함께 검증하세요. Raft/distributed code를 바꾸면
all-features distributed E2E를 실행하되 component test 통과를 production-ready cluster로
표현하지 마세요. auth/session 변경은 HTTP와 gRPC, role boundary, compound/`PROFILE`,
session invalidation을 모두 검증하고 credential을 log나 error에 넣지 마세요.

## 문서와 PR

영문 `book/src/`가 canonical입니다. public 영문 문서를 변경하면 대응하는
`book/src.ko/` 문서도 같은 PR에서 갱신하고 command, identifier, feature status,
limitation을 의미상 맞추세요.

PR에는 문제, 선택한 동작, security/compatibility/data-migration 영향, 실행한 정확한
test command와 결과, 관련 issue를 적으세요. credential, `.env`, generated database,
machine-specific build output은 commit하지 않습니다.
