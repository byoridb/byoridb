[한국어](../ko/getting-started/quickstart.html)

# Quickstart

This walkthrough starts a local standalone server, connects with the simple CLI
REPL, and creates a small graph.

## Prerequisites

- Rust 1.90 through [rustup](https://rustup.rs/)
- `protoc` (`protobuf-compiler`)
- A cloned ByoriDB repository

See [Installation](./installation.md) for platform-specific setup.

## Build and start the server

```bash
cargo build --locked --workspace --release
export BYORIDB_ROOT_PASSWORD='replace-with-a-long-random-secret'
cargo run --locked --release -p byoridb --bin byoridb-server
```

The standalone launcher combines the storage and graph layers in one process.
Its current defaults are:

| Interface | Default listener |
| --- | --- |
| gRPC | `0.0.0.0:9669` |
| HTTP | `0.0.0.0:19669` |

Both listeners are plaintext. Keep them on a trusted network, or bind to
loopback as shown in [Configuration](./configuration.md). The server requires a
non-empty `BYORIDB_ROOT_PASSWORD`; the value is not printed to logs.

## Connect with the CLI

Open another terminal in the repository:

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='same-secret-used-to-start-the-server'
cargo run --locked --release -p byoridb-client --bin byoridb-cli
```

The prompt is `byoridb>`. Enter one logical query per line.

## Create and query a graph

Run these statements in order:

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

Type `quit` or `exit` to close the CLI.

## Next steps

- [Configuration](./configuration.md)
- [CLI](../guide/cli.md)
- [nGQL syntax](../guide/ngql-syntax.md)
- [Users and roles](../guide/ngql/users.md)
