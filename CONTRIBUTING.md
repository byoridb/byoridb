# ByoriDB에 기여하기

ByoriDB에 기여하는 데 관심을 가져주셔서 감사합니다! 커뮤니티의 기여를 환영합니다.

## 시작하기

### 사전 요구사항

- **Rust**: `rust-toolchain.toml`에 고정된 Rust 1.90을 사용합니다. [rustup](https://rustup.rs/)으로 설치하세요.
- **protobuf-compiler**: tonic/gRPC code generation에 필요합니다.
- C++ 빌드 도구가 필요 없습니다 — 스토리지는 순수 Rust(redb)로 구현되었습니다.

### 프로젝트 빌드하기

1. 저장소를 클론합니다:
   ```bash
   git clone https://github.com/byoridb/byoridb.git
   cd byoridb
   ```

2. 프로젝트를 빌드합니다:
   ```bash
   cargo build
   ```

3. git 훅을 설정합니다 (권장):
   ```bash
   ./scripts/setup-hooks.sh
   ```
   이렇게 하면 코드 포매팅을 자동으로 검사하는 pre-commit 훅이 활성화됩니다.

### 테스트 실행하기

Pull Request를 제출하기 전에 전체 workspace 테스트를 직렬로 실행합니다. redb 파일
락과 임시 DB 경합을 피하기 위해 `--test-threads=1`이 필요합니다.

```bash
cargo test --workspace --all-features -- --test-threads=1
```

특정 테스트를 실행하려면:

```bash
cargo test --package byoridb-executor test_name
```

## 코드 스타일

표준 Rust 코딩 컨벤션을 따릅니다.

- **포매팅**: 코드를 `rustfmt`로 포매팅했는지 확인해 주세요.
  ```bash
  cargo fmt --all
  ```

- **Clippy**: 흔한 실수를 잡기 위해 clippy를 실행하세요.
  ```bash
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  ```

## 개발 워크플로

1. 저장소를 포크합니다.
2. 기능 또는 수정 사항을 위한 새 브랜치를 생성합니다 (`git checkout -b feature/my-feature`).
3. `<type>(<scope>): <subject>` 형식으로 커밋합니다
   (`git commit -am 'fix(executor): preserve temporal history'`).
4. 브랜치에 푸시합니다 (`git push origin feature/my-feature`).
5. Pull Request를 엽니다.

## 프로젝트 구조

- `byoridb-common`: 핵심 데이터 타입 (Value, Vertex, Edge, DataSet).
- `byoridb-kvstore`: KV 스토리지 계층 (redb, 순수 Rust).
- `byoridb-codec`: 스키마 버저닝을 지원하는 행(row) 인코딩/디코딩.
- `byoridb-storage`: 스토리지 서비스, Raft 합의, 인덱싱.
- `byoridb-meta`: 메타데이터 관리, 파티션 할당.
- `byoridb-parser`: nGQL 쿼리 언어 파서.
- `byoridb-executor`: 쿼리 계획 수립 및 실행 엔진.
- `byoridb`: Graph 서비스, HTTP/gRPC 서버.
- `byoridb-client`: 클라이언트 라이브러리 및 CLI.

## 도움 받기

질문이 있으면 이슈를 열거나 커뮤니티 토론에 참여해 주세요.
