# 설치

## 시스템 요구사항

### 지원 플랫폼
- Linux (Ubuntu 20.04+, CentOS 7+, Debian 10+)
- macOS (10.15+)

> **참고:** Windows는 현재 지원되지 않습니다.

### 하드웨어 요구사항
- CPU: 2코어 이상 권장
- 메모리: 4GB 이상 권장
- 디스크: 프로덕션 환경에서는 SSD 권장

### 소프트웨어 의존성
- Rust 1.90 이상
- protobuf-compiler (gRPC 코드 생성용)
- pkg-config

스토리지 엔진은 순수 Rust(redb)이므로 **C++ 툴체인**(cmake/clang)이 필요하지
않습니다. `build-essential`/`pkg-config`로 몇 안 되는 네이티브 크레이트(zstd, openssl)를 처리할 수 있습니다.

## 의존성 설치

### Ubuntu/Debian

```bash
sudo apt update
sudo apt install -y build-essential pkg-config protobuf-compiler
```

### macOS

```bash
xcode-select --install
brew install protobuf
```

### CentOS/RHEL

```bash
sudo yum groupinstall -y "Development Tools"
sudo yum install -y protobuf-compiler
```

## Rust 설치

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

설치 확인:

```bash
rustc --version
cargo --version
```

## ByoriDB 빌드

### 저장소 클론

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
```

### 디버그 빌드

```bash
cargo build
```

### 릴리스 빌드 (권장)

```bash
cargo build --release
```

릴리스 빌드는 더 나은 성능을 위해 LTO(Link-Time Optimization)를 활성화합니다.

### 빌드 산출물

빌드 후 다음 항목을 찾을 수 있습니다:

| 바이너리 | 위치 | 설명 |
|--------|----------|-------------|
| `byoridb-server` | `target/release/` | 독립 실행형 서버 |
| `byoridb-cli` | `target/release/` | CLI 클라이언트 |

## 설치 확인

```bash
# Run tests
cargo test

# Start server
./target/release/byoridb-server

# In another terminal, connect with CLI
./target/release/byoridb-cli
```

## 문제 해결

### Protobuf 컴파일러를 찾을 수 없음

gRPC 빌드가 `protoc` 누락으로 실패하는 경우:

```bash
# Ubuntu/Debian
sudo apt install -y protobuf-compiler

# macOS
brew install protobuf
```

### 링킹 오류

```bash
# Ensure pkg-config can find libraries
export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig
```

### 빌드 중 메모리 부족

```bash
# Limit parallel jobs
cargo build --release -j 2
```
