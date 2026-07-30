# 빠른 시작

5분 안에 ByoriDB를 실행해 봅니다.

## 사전 요구사항

- Rust 1.90+ ([rustup](https://rustup.rs/)을 통해 설치)
- protobuf-compiler (gRPC 코드 생성용)

## 소스에서 빌드

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --locked --release
```

## 서버 시작

embedded Storage와 Graph gRPC/HTTP를 한 프로세스에서 실행합니다. Meta gRPC는
`cluster.peers`를 설정한 경우에만 함께 시작됩니다:

```bash
BYORIDB_ROOT_PASSWORD='<root-password>' cargo run --locked --bin byoridb-server --release
```

서버는 다음에서 시작됩니다:
- gRPC: `localhost:9669`
- HTTP: `localhost:19669`

## CLI로 연결

새 터미널에서:

```bash
BYORIDB_USER=root BYORIDB_PASSWORD='<root-password>' \
  cargo run --locked -p byoridb-client --bin byoridb-cli
```

## 첫 번째 그래프

```sql
-- Create a space
CREATE SPACE social(vid_type=INT64);
USE social;

-- Define schema
CREATE TAG person(name STRING, age INT64);
CREATE EDGE knows(since INT64);

-- Insert vertices
INSERT VERTEX person(name, age) VALUES 1:('Alice', 30);
INSERT VERTEX person(name, age) VALUES 2:('Bob', 25);
INSERT VERTEX person(name, age) VALUES 3:('Carol', 28);

-- Insert edges
INSERT EDGE knows(since) VALUES 1->2:(2020);
INSERT EDGE knows(since) VALUES 2->3:(2021);

-- Query: Who does Alice know?
GO FROM 1 OVER knows YIELD $$.person.name;

-- Query: Find path from Alice to Carol
FIND SHORTEST PATH FROM 1 TO 3 OVER knows;
```

## 다음 단계

- [설치](./installation.md) - 자세한 설치 안내
- [설정](./configuration.md) - 필요에 맞게 ByoriDB 구성하기
- [nGQL 문법](../guide/ngql-syntax.md) - 쿼리 언어 배우기
