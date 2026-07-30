# 기여하기

ByoriDB에 대한 기여를 환영합니다.

## 시작하기

### 사전 요구 사항

- Rust 1.90+
- protobuf-compiler (gRPC 코드 생성용)
- Git

### 설정

```bash
# Clone the repository
git clone https://github.com/byoridb/byoridb.git
cd byoridb

# Setup git hooks
./scripts/setup-hooks.sh

# Build
cargo build --locked

# Run tests
cargo test --locked
```

## 개발 워크플로

### 브랜치 전략

- `main` - 통합 및 릴리스 기준 브랜치
- `feature/*`, `fix/*`, `docs/*` - 작업 브랜치

### 기능 브랜치 생성

```bash
git checkout main
git pull origin main
git checkout -b feature/my-feature
```

### 변경 작업

1. 코드를 작성합니다
2. 테스트를 추가합니다
3. 검사를 실행합니다:

```bash
# Format code
cargo fmt --all

# Run linter
cargo clippy --locked --all-targets --all-features -- -D warnings

# Run tests
cargo test --locked
```

### 사전 커밋(Pre-commit) 훅

저장소에는 포매팅을 검사하는 사전 커밋 훅이 포함되어 있습니다:

```bash
# Runs automatically on commit
cargo fmt -- --check
```

훅이 실패하면 다음을 실행하세요:

```bash
cargo fmt --all
git add -A
git commit
```

### 커밋 메시지

conventional commits를 사용하세요:

```
type(scope): description

[optional body]

[optional footer]
```

타입:
- `feat` - 새 기능
- `fix` - 버그 수정
- `docs` - 문서
- `style` - 포매팅
- `refactor` - 코드 구조 개선
- `test` - 테스트 추가
- `chore` - 유지보수

예시:

```
feat(parser): add ALTER TAG statement support
fix(storage): resolve race condition in batch write
docs(readme): update installation instructions
```

### 풀 리퀘스트(Pull Request)

1. 브랜치를 푸시합니다:

```bash
git push origin feature/my-feature
```

2. `main`을 대상으로 PR을 생성합니다
3. PR 템플릿을 작성합니다
4. CI 검사를 기다립니다
5. 리뷰 코멘트를 반영합니다
6. 승인 후 병합합니다

## 코드 스타일

### Rust 가이드라인

- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)를 따르세요
- 포매팅에 `rustfmt`를 사용하세요
- 린팅에 `clippy`를 사용하세요

### 문서화

- 공개 API에 doc 주석을 추가하세요
- 문서에 예제를 포함하세요

```rust
/// Executes a query and returns results.
///
/// # Arguments
///
/// * `query` - The nGQL query string
///
/// # Example
///
/// ```
/// let result = executor.execute("SHOW SPACES")?;
/// ```
pub fn execute(&self, query: &str) -> Result<DataSet> {
    // ...
}
```

### 오류 처리

- 애플리케이션 코드에는 `anyhow::Result`를 사용하세요
- 라이브러리 오류 타입에는 `thiserror`를 사용하세요
- 오류에 컨텍스트를 추가하세요

```rust
use anyhow::{Context, Result};

fn read_config(path: &Path) -> Result<Config> {
    let content = fs::read_to_string(path)
        .context("Failed to read config file")?;

    toml::from_str(&content)
        .context("Failed to parse config")
}
```

## 테스트

### 단위 테스트

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_create_space() {
        let query = "CREATE SPACE test(vid_type=INT64)";
        let ast = parse(query).unwrap();
        assert!(matches!(ast, Statement::CreateSpace(_)));
    }
}
```

### 통합 테스트

`tests/` 디렉터리에 배치합니다:

```rust
// tests/integration_test.rs

#[tokio::test]
async fn test_full_workflow() {
    let server = TestServer::start().await;
    let client = Client::connect(&server.addr()).await?;

    client.execute("CREATE SPACE test(vid_type=INT64)").await?;
    // ...
}
```

### 테스트 실행

```bash
# All tests
cargo test --locked

# Specific package
cargo test --locked --package byoridb-parser

# Specific test
cargo test --locked --package byoridb-parser test_parse_alter
```

## 아키텍처 결정

중요한 변경의 경우 RFC를 작성하세요:

1. `docs/rfcs/template.md`를 복사합니다
2. 제안 내용을 작성합니다
3. PR로 제출합니다
4. PR 코멘트에서 논의합니다
5. 피드백을 바탕으로 수정합니다
6. 승인 후 구현합니다

## 도움 받기

- 버그는 이슈를 열어 주세요
- 질문은 디스커션을 시작해 주세요
- 커뮤니티 채팅에 참여하세요
