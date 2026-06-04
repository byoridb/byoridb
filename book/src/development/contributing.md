# Contributing

We welcome contributions to ByoriDB.

## Getting Started

### Prerequisites

- Rust 1.90+
- protobuf-compiler (for gRPC codegen)
- Git

### Setup

```bash
# Clone the repository
git clone https://github.com/byoridb/byoridb.git
cd byoridb

# Setup git hooks
./scripts/setup-hooks.sh

# Build
cargo build

# Run tests
cargo test
```

## Development Workflow

### Branch Strategy

- `main` - Stable releases
- `develop` - Development branch
- `feature/*` - Feature branches
- `fix/*` - Bug fix branches

### Creating a Feature Branch

```bash
git checkout develop
git pull origin develop
git checkout -b feature/my-feature
```

### Making Changes

1. Write code
2. Add tests
3. Run checks:

```bash
# Format code
cargo fmt --all

# Run linter
cargo clippy --all-targets --all-features -- -D warnings

# Run tests
cargo test
```

### Pre-commit Hook

The repository includes a pre-commit hook that checks formatting:

```bash
# Runs automatically on commit
cargo fmt -- --check
```

If the hook fails, run:

```bash
cargo fmt --all
git add -A
git commit
```

### Commit Messages

Use conventional commits:

```
type(scope): description

[optional body]

[optional footer]
```

Types:
- `feat` - New feature
- `fix` - Bug fix
- `docs` - Documentation
- `style` - Formatting
- `refactor` - Code restructuring
- `test` - Adding tests
- `chore` - Maintenance

Examples:

```
feat(parser): add ALTER TAG statement support
fix(storage): resolve race condition in batch write
docs(readme): update installation instructions
```

### Pull Requests

1. Push your branch:

```bash
git push origin feature/my-feature
```

2. Create PR against `develop`
3. Fill in the PR template
4. Wait for CI checks
5. Address review comments
6. Merge after approval

## Code Style

### Rust Guidelines

- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `rustfmt` for formatting
- Use `clippy` for linting

### Documentation

- Add doc comments for public APIs
- Include examples in documentation

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

### Error Handling

- Use `anyhow::Result` for application code
- Use `thiserror` for library error types
- Add context to errors

```rust
use anyhow::{Context, Result};

fn read_config(path: &Path) -> Result<Config> {
    let content = fs::read_to_string(path)
        .context("Failed to read config file")?;

    toml::from_str(&content)
        .context("Failed to parse config")
}
```

## Testing

### Unit Tests

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

### Integration Tests

Place in `tests/` directory:

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

### Running Tests

```bash
# All tests
cargo test

# Specific package
cargo test --package byoridb-parser

# Specific test
cargo test --package byoridb-parser test_parse_alter
```

## Architecture Decisions

For significant changes, create an RFC:

1. Copy `docs/rfcs/template.md`
2. Fill in the proposal
3. Submit as PR
4. Discuss in PR comments
5. Revise based on feedback
6. Implement after approval

## Getting Help

- Open an issue for bugs
- Start a discussion for questions
- Join our community chat
