# Quick Start

Get ByoriDB running in under 5 minutes.

## Prerequisites

- Rust 1.90+ (install via [rustup](https://rustup.rs/))
- protobuf-compiler (for gRPC codegen)

## Build from Source

```bash
git clone https://github.com/byoridb/byoridb.git
cd byoridb
cargo build --release
```

## Start the Server

Run the standalone server (Meta + Storage + Graph services combined):

```bash
cargo run --bin byoridb-server --release
```

The server starts on:
- gRPC: `localhost:9669`
- HTTP: `localhost:19669`

## Connect with CLI

In a new terminal:

```bash
cargo run -p byoridb-client --bin byoridb-cli
```

## Your First Graph

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

## Next Steps

- [Installation](./installation.md) - Detailed installation instructions
- [Configuration](./configuration.md) - Configure ByoriDB for your needs
- [nGQL Syntax](../guide/ngql-syntax.md) - Learn the query language
