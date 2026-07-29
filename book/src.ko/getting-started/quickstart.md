[English](../../getting-started/quickstart.html)

# 빠른 시작

로컬 스탠드얼론 서버를 시작하고 간단한 CLI REPL로 접속한 뒤 작은 그래프를
만들어 봅니다.

## 사전 요구사항

- [rustup](https://rustup.rs/)으로 설치한 Rust 1.90
- `protoc`(`protobuf-compiler`)
- 복제한 ByoriDB 저장소

운영체제별 준비는 [설치](./installation.md)를 참고하세요.

## 빌드 및 서버 시작

```bash
cargo build --workspace --release
export BYORIDB_ROOT_PASSWORD='replace-with-a-long-random-secret'
cargo run --release --bin byoridb-server
```

스탠드얼론 런처는 저장소와 그래프 계층을 한 프로세스에 결합합니다. 현재 기본
리스너는 다음과 같습니다.

| 인터페이스 | 기본 리스너 |
| --- | --- |
| gRPC | `0.0.0.0:9669` |
| HTTP | `0.0.0.0:19669` |

두 리스너 모두 평문입니다. 신뢰할 수 있는 네트워크에서만 사용하거나
[설정](./configuration.md)처럼 루프백에 바인딩하세요. 서버에는
`BYORIDB_ROOT_PASSWORD`가 필요하며 그 값은 로그에 출력되지 않습니다.

## CLI 접속

저장소의 새 터미널에서 실행합니다.

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD="$BYORIDB_ROOT_PASSWORD"
cargo run --release -p byoridb-client --bin byoridb-cli
```

프롬프트는 `byoridb>`입니다. 한 줄에 하나의 논리적 쿼리를 입력하세요.

## 그래프 생성과 조회

아래 문을 순서대로 실행합니다.

```sql
CREATE SPACE social (vid_type = INT64);
USE social;

CREATE TAG person(name STRING, age INT64);
CREATE EDGE knows(since INT64);

INSERT VERTEX person(name, age) VALUES 1:("Alice", 30), 2:("Bob", 25), 3:("Carol", 28);
INSERT EDGE knows(since) VALUES 1->2:(2020), 2->3:(2021);

FETCH PROP ON person 1, 2, 3;
GO FROM 1 OVER knows YIELD knows._dst AS friend_id;
MATCH (p:person) WHERE p.person.age >= 28 RETURN p.person.name AS name, p.person.age AS age;
FIND SHORTEST PATH FROM 1 TO 3 OVER knows;
```

CLI를 닫으려면 `quit` 또는 `exit`를 입력합니다.

## 다음 단계

- [설정](./configuration.md)
- [CLI](../guide/cli.md)
- [nGQL 문법](../guide/ngql-syntax.md)
- [사용자와 역할](../guide/ngql/users.md)
