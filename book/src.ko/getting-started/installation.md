[English](../../getting-started/installation.html)

# 설치

ByoriDB는 현재 소스에서 빌드합니다. 저장소가 Rust 1.90을 고정하므로 `rustup`을
사용하면 알맞은 컴파일러가 자동으로 선택됩니다.

## 지원 운영체제

- Linux
- macOS

Windows는 현재 지원하지 않습니다. ByoriDB의 저장소 엔진은 순수 Rust 기반
`redb`이므로 RocksDB나 RocksDB용 C++ 툴체인은 필요하지 않습니다.

Release workflow는 새 tag archive의 root에 `LICENSE`와 `NOTICES.md`를 포함하고,
게시 전에 두 파일이 모두 있는지 검증합니다. 이미 게시된 v0.3.3 이하 archive는 이
검증이 도입되기 전에 만들어졌으며 소급 변경되지 않습니다. 해당 legacy archive를
재배포하기 전에 저장소의 license와 notices를 확인하세요.

## 빌드 요구사항

- Git
- Cargo, rustfmt, Clippy를 포함한 Rust 1.90
- gRPC 코드 생성에 필요한 `protoc`(`protobuf-compiler`)
- 일부 전이 의존성을 컴파일할 때 사용할 운영체제의 기본 C 빌드 도구

Ubuntu 또는 Debian에서는 다음과 같이 설치합니다.

```bash
sudo apt update
sudo apt install -y build-essential protobuf-compiler
```

macOS에서는 다음과 같이 설치합니다.

```bash
xcode-select --install
brew install protobuf
```

Rust가 없다면 [rustup](https://rustup.rs/)으로 설치합니다.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

## ByoriDB 빌드

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --locked --workspace --release
```

주요 바이너리는 다음과 같습니다.

| 바이너리 | 출력 경로 | 용도 |
| --- | --- | --- |
| `byoridb-server` | `target/release/byoridb-server` | 스탠드얼론 서버 |
| `byoridb-cli` | `target/release/byoridb-cli` | gRPC 명령줄 클라이언트 |
| `byoridb-backup` | `target/release/byoridb-backup` | 백업 유틸리티 |

서버 또는 CLI만 빌드할 수도 있습니다.

```bash
cargo build --locked --release --bin byoridb-server
cargo build --locked --release -p byoridb-client --bin byoridb-cli
```

## 체크아웃 검증

임시 redb 데이터베이스의 파일 잠금 경합을 피하려면 통합 테스트를 직렬로
실행해야 합니다.

```bash
cargo test --locked --workspace --all-features -- --test-threads=1
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

## 서버 시작

스탠드얼론 바이너리는 비어 있지 않은 root 비밀번호가 없으면 시작하지 않습니다.
공백으로만 구성된 값에 대한 별도 강도 검사는 현재 수행하지 않습니다. 복구 가능한
임의 비밀번호를 생성해 로그에 출력하지도 않습니다.

```bash
export BYORIDB_ROOT_PASSWORD='replace-with-a-long-random-secret'
./target/release/byoridb-server
```

개발 장비 밖에 서버를 노출하기 전에 [설정](./configuration.md)을 확인하세요.
내장 리스너는 TLS를 제공하지 않습니다.

## 문제 해결

빌드가 `protoc` 누락을 보고하면 Linux에서는 `protobuf-compiler`, macOS에서는
Homebrew의 `protobuf`를 설치하세요. 릴리스 빌드 중 메모리가 부족하면 Cargo
병렬도를 낮출 수 있습니다.

```bash
cargo build --locked --workspace --release -j 2
```
