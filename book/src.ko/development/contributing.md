# 기여하기

[English](../../development/contributing.html) | **한국어**

ByoriDB 기여를 환영합니다. 이 페이지는 저장소에서 실제로 적용하는 작업 절차를
요약합니다. 루트의
[CONTRIBUTING.ko.md](https://github.com/byoridb/byoridb/blob/main/CONTRIBUTING.ko.md)도
읽으세요. 로컬 체크아웃에 `AGENTS.md`가 있으면 AI를 활용한 변경 시 해당 저장소의
에이전트 지침을 따르세요. 보안 취약점은 공개 이슈에 올리지 말고
[SECURITY.ko.md](https://github.com/byoridb/byoridb/blob/main/SECURITY.ko.md)의 비공개
보고 절차를 따르세요.

## Agent-first 수용 기준

[제품 설계](../architecture/agent-first.html)를 따르고 동작 변경을
[로드맵](roadmap.html)의 작업과 연결하세요. 변경이 영향을 미치는 discovery, 손실 없는
값, 제한·완전성이 명시된 결과, 근거, 변경 결과, 인가를 평가합니다.
HTTP/gRPC/SDK/MCP adapter는 엔진 의미를 공유합니다.

Agent-facing 변경은 재현 가능한 작업, 예상 답변·근거, 최종 DB 상태를 기록합니다.
계획 중인 EVAL-1 harness가 생기면 해당 fixture를 추가하고, 그 전에는 Agent 평가를
미실행으로 표시합니다. 호출·토큰·지연보다 작업 정확성을 먼저 측정합니다. 운영자용
문서·CLI 변경은 일반적인 사용성 검사도 계속 적용합니다.

## 사전 준비

- Git
- `rust-toolchain.toml`이 선택하는 Rust 1.90
- gRPC 코드 생성용 `protobuf-compiler`(`protoc`)
- Linux 또는 macOS

운영 KV 엔진은 순수 Rust로 구현되어 있으므로 RocksDB/C++는 필요하지 않습니다.

## 체크아웃 준비

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb

# Make the repository's formatting hook active.
bash scripts/setup-hooks.sh

cargo build --locked --workspace
```

현재 `main`에서 목적이 분명한 브랜치를 만들고 PR도 `main`을 대상으로 여세요.
활성 `develop` 브랜치는 없으므로 이를 전제로 작업 절차를 구성하지 마세요.

## 워크스페이스 구성

변경하려는 동작을 담당하는 크레이트를 선택하세요.

| 변경 | 위치 |
|---|---|
| 공통 값과 그래프 타입 | `byoridb-common` |
| 영속 KV 동작과 백업 구현 | `byoridb-kvstore` |
| 정점·간선·행 인코딩 | `byoridb-codec` |
| 저장소·인덱스·파티션·Raft | `byoridb-storage` |
| 메타데이터 서비스 | `byoridb-meta` |
| 렉서·AST·문법 | `byoridb-parser` |
| 쿼리 계획과 실행 | `byoridb-executor` |
| 인증·세션·gRPC·HTTP | `byoridb-graph` |
| Rust 클라이언트와 CLI | `byoridb-client` |
| 오프라인 가져오기 | `byoridb-bulkloader` |
| 서버 또는 백업 바이너리 | 루트 `src/` |

삭제된 `byoridb-core` 크레이트를 다시 만들지 마세요.
`byoridb-executor/src/executor/mod.rs`의 실행 라우터는 작게 유지하고,
새 실행 로직은 목적별 모듈에 추가하세요.

## 코드 규칙

- 함수·변수·모듈은 `snake_case`, 타입은 `PascalCase`, 상수는
  `SCREAMING_SNAKE_CASE`를 사용합니다.
- 라이브러리 크레이트의 오류는 타입 있는 `thiserror`로 정의합니다.
  서비스·바이너리 경계에서는 `anyhow`로 문맥을 추가합니다.
- 운영 코드에 `unwrap()`이나 `expect()`를 추가하지 않습니다.
- 운영 코드에서는 `println!`, `eprintln!`, `dbg!` 대신 구조화된 `tracing`
  필드를 사용합니다.
- 의존성은 먼저 루트 `[workspace.dependencies]`에 등록하고, 멤버 크레이트에서
  `workspace = true`로 참조합니다.
- 새 동작에는 인라인 단위 테스트를 추가하고, 크레이트 간 경로가 중요하면
  통합 테스트도 추가합니다.

라이브러리 오류 예시:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("space not selected")]
    NoSpace,
}
```

구조화된 로그 예시:

```rust
tracing::info!(space = %space_name, "query executed");
```

## 필수 로컬 검사

PR에 요구되는 전체 검사를 로컬에서도 실행하세요.

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --workspace --release
```

임시 redb 데이터베이스의 파일 경합을 피하려면 통합 테스트를 순차 실행해야 합니다.
개발 중에는 다음과 같이 범위를 좁힌 명령을 사용할 수 있습니다.

```bash
cargo test --locked --package byoridb-parser
cargo test --locked --package byoridb-executor <test-name>
```

GitHub `CI` 워크플로는 버전이 고정된 RustSec 의존성 감사도 실행합니다. 해당
release-build 작업은 check·format·Clippy·test·audit가 모두 통과할 때까지 기다립니다.
별도 Auto-release도 버전 변경 전과 rebase 이후를 포함한 모든 push 후보를
감사합니다. 릴리스 워크플로는 서명 비밀값 preflight 다음에 정확한 release ref를
감사하고 통과한 뒤 빌드·발행합니다.

## 주의가 필요한 영역

일부 모듈에는 추가 회귀 검사가 필요합니다.

### 쿼리 정확성 회귀

`docs/PLAN.md`의 H 계열 경로를 변경하면 다중 패턴 MATCH 테스트를 포함해
H-1부터 H-6까지의 회귀 검사를 다시 실행하세요.
`space_id`, `tag_id`, `edge_id`를 실패 시 0으로 대체하는 동작을 다시 도입하지 마세요.

### 시계열 저장

현재·이력 저장소, 시계열 DML, 파싱, 계획을 변경할 때는 현재 뷰와 이력을
모두 보존해야 합니다.

```bash
cargo test --locked -p byoridb-kvstore --test temporal
cargo test --locked -p byoridb-parser fetch_as_of
cargo test --locked -p byoridb-executor fetch_as_of
```

이어서 전체 워크스페이스 테스트를 순차 실행하세요. 해당되는 경우 정점·간선의
`AS OF`, tombstone, 동일 밀리초의 쓰기, 현재·이력의 원자적 적용을 검증하세요.

### Raft와 분산 코드

`byoridb-storage/src/raft/`를 변경하기 전에 자체 Raft 상태 머신, 로그, 스냅샷,
멤버십, 네트워크 드라이버를 이해하세요. 다음을 실행합니다.

```bash
cargo test --locked -p byoridb --all-features \
  --test distributed_e2e_test -- --test-threads=1
```

구성 요소 테스트를 통과했다고 해서 미완성 클러스터 실행기가 운영 가능한 배포가
되는 것은 아닙니다. 그 경계를 문서에 명시하세요.

### 인증과 인가

사용자·역할·세션·프로토콜을 변경하면 HTTP와 gRPC 동작, 영속 사용자 캐시 동기화,
세션 무효화, 복합 문장, `PROFILE` 인가를 검사하세요. 로그, 메트릭, 진단,
오류 응답에 자격 증명이나 가공하지 않은 세션 ID를 포함하지 마세요.

## 문서 변경

`book/src/`의 영문 페이지가 기준입니다. 대응하는 `book/src.ko/`의 한국어 페이지를
유지하고 명령, 기능 상태, 제한 사항의 의미를 맞추세요. 재현 가능한 환경과
결과 산출물 없이 측정 성능 수치를 게시하지 마세요.

mdBook이 설치되어 있으면 저장소의 문서 워크플로·설정으로 두 언어 트리를
모두 빌드하세요.

## Pull request

PR은 범위를 명확히 하고 다음을 포함하세요.

- 문제와 선택한 동작
- 보안·호환성·데이터 마이그레이션 영향
- 추가한 테스트와 실행한 정확한 명령
- 사용자에게 보이는 동작에 대한 문서 갱신
- 해당되는 관련 이슈 링크

`.env` 파일, 자격 증명, 생성된 데이터베이스 파일, 특정 컴퓨터의 빌드 산출물을
커밋하지 마세요. PR 템플릿과 CI 결과가 제출 체크리스트의 기준입니다.
