# ByoriDB 기여 가이드

[English (기본)](CONTRIBUTING.md) | [한국어](CONTRIBUTING.ko.md)

ByoriDB 개선에 참여해 주셔서 감사합니다. Bug report, 범위가 명확한 fix, test 및
documentation 개선을 환영합니다.

취약점으로 의심되는 내용은 public issue로 열지 마세요.
[SECURITY.ko.md](SECURITY.ko.md)의 비공개 제보 절차를 따르세요.

## 시작하기 전에

- 중복 작업을 피하기 위해 기존 issue와 Pull Request를 검색하세요.
- 큰 기능, storage format 변경, query language 변경 또는 distributed system 변경은
  먼저 issue를 열고 제안 동작 및 compatibility 영향을 설명하세요.
- 현재 구현 제약과 알려진 regression 영역은 [docs/PLAN.ko.md](docs/PLAN.ko.md)를
  확인하세요.
- 변경 범위를 집중해서 유지하세요. 관련 없는 formatting, refactor, feature 작업을
  한 Pull Request에 섞지 마세요.

## 개발 환경

필수 도구:

- Linux 또는 macOS
- `rust-toolchain.toml`에 고정된 Rust 1.90
- gRPC code generation을 위한 `protobuf-compiler`

embedded storage backend는 redb이므로 C++ toolchain이 필요하지 않습니다.

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --locked --workspace
```

원한다면 저장소의 formatting pre-commit hook을 활성화하세요.

```bash
./scripts/setup-hooks.sh
```

## 저장소 구조

| 경로 | 책임 |
|---|---|
| `src/` | `byoridb-server`, `byoridb-backup` entry point |
| `byoridb-common/` | 공용 graph value, record, dataset, crypto helper |
| `byoridb-kvstore/` | redb current view, temporal history, backup 지원 |
| `byoridb-codec/` | Binary row encoding/decoding |
| `byoridb-storage/` | Vertex/edge storage, partitioning, custom Raft |
| `byoridb-meta/` | Space, schema, partition 및 metadata service |
| `byoridb-parser/` | Lexer, AST 및 nGQL 스타일 parser |
| `byoridb-executor/` | Plan, query 실행, inference, path, recommendation |
| `byoridb-graph/` | HTTP/gRPC, 인증, session 및 query 조정 |
| `byoridb-client/` | Rust client와 `byoridb-cli` |
| `byoridb-bulkloader/` | Bulk-loading binary와 library |
| `tests/` | Workspace integration 및 distributed test |
| `book/` | mdBook 사용자 및 운영 문서 |
| `docs/` | 구현 상태, 결정 및 migration plan |

삭제된 `byoridb-core/` crate를 다시 만들지 마세요.

## 코드 convention

- Rust naming convention을 따릅니다. Function/module은 `snake_case`, type은
  `PascalCase`, 상수는 `SCREAMING_SNAKE_CASE`를 사용합니다.
- Library crate는 `thiserror`로 typed error를 제공하고 service/binary 경계는
  `anyhow`로 context를 추가할 수 있습니다.
- Production path에 `unwrap()`이나 `expect()`를 추가하지 마세요. 실패 message가
  유용한 test code에서는 사용할 수 있습니다.
- Production code는 structured `tracing` event를 사용합니다. Production path에
  `println!`, `eprintln!`, `dbg!`를 commit하지 마세요.
- 공용 third-party dependency는 root `Cargo.toml`의 `[workspace.dependencies]`에
  등록한 뒤 member crate에서 `workspace = true`로 참조하세요.
- 새 동작에 집중된 test를 추가하세요. Unit behavior는 inline `#[cfg(test)]` module,
  cross-crate behavior는 `tests/`를 우선합니다.
- `byoridb-executor/src/executor/mod.rs`는 router로 유지하고 큰 새 실행 logic은
  목적별 module에 배치하세요.
- Credential, `.env` 파일, private data, 생성된 database를 commit하지 마세요.

## 문서 convention

영어가 canonical documentation language입니다. Public 영문 문서를 바꾸면 같은 Pull
Request에서 한국어 mirror(`*.ko.md`)도 갱신하세요. Command, identifier, path, query
example은 두 언어에서 동일하게 유지합니다.

현재 코드에 존재하는 동작을 문서화하세요. 미완성 또는 experimental behavior는
명확히 표시하며 특히 현재 multi-node launcher를 production-ready로 설명하지 마세요.

## 필수 검증

전체 workspace를 check, format, lint합니다.

```bash
cargo check --locked --workspace --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

모든 test를 직렬 실행한 뒤 release workspace build를 확인합니다. 임시 redb
database가 file/lock을 두고 경합할 수 있어 직렬 실행이 필수입니다.

```bash
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
```

CI는 commit된 `Cargo.lock`을 대상으로 RustSec dependency audit도 실행합니다.

작업 중 사용할 수 있는 좁은 명령:

```bash
cargo test --locked --package byoridb-executor
cargo test --locked --package byoridb-graph
cargo test --locked --package byoridb-kvstore --test temporal
```

최종 local gate는 여전히 전체 workspace test와 release build 명령입니다.

### 고위험 영역

- **Space/tag/edge identifier:** schema key, planning, lookup, fetch, traversal 또는
  multi-pattern matching을 바꾸면 `docs/PLAN.ko.md`의 H-series regression을 확인하고
  관련 regression test를 재실행하세요.
- **Temporal path:** current-view 동작과 history를 함께 검증하세요. Temporal KV test,
  parser/executor `fetch_as_of` test 및 전체 serial workspace suite를 실행하세요.
- **Custom Raft:** `byoridb-storage/src/raft/` 변경 전 log, snapshot, membership 동작을
  이해하고 all features로 `tests/distributed_e2e_test.rs`를 실행하세요.
- **인증과 권한:** nested/compound statement 및 session invalidation을 포함해 모든
  privilege boundary에 negative test를 추가하세요.

## Pull Request workflow

1. 저장소를 fork하고 범위가 명확한 branch를 만듭니다.
2. Test와 문서를 포함한 가장 작은 coherent change를 구현합니다.
3. 위 필수 검증을 실행합니다.
4. `fix(executor): preserve temporal history`와 같은 간결한 conventional commit을
   사용합니다.
5. Branch를 push하고 저장소 template으로 Pull Request를 엽니다.

Pull Request에는 다음을 설명하세요.

- 문제와 의도한 동작
- 중요한 설계 선택과 compatibility risk
- 실행한 command와 결과
- Migration, deployment 또는 rollback 고려사항
- 구현과 함께 변경한 문서

별도 명시가 없다면 모든 contribution은 저장소의
[Apache License 2.0](LICENSE)에 따라 제출됩니다.

## 도움 받기

재현 가능한 bug와 범위가 명확한 feature 논의는 GitHub issue를 여세요. ByoriDB
revision, 운영체제, secret을 제거한 관련 설정, 최소 query sequence, 관찰한 error를
포함하세요. Credential, data 또는 access-control bypass를 노출할 가능성이 있으면
반드시 비공개 security 절차를 사용하세요.
