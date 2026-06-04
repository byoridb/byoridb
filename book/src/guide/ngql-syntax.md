# nGQL Syntax Guide

ByoriDB uses nGQL (Graph Query Language), a SQL-like query language for graph databases.

## Overview

nGQL supports three categories of statements:

- **DDL (Data Definition Language)**: Define schemas and spaces
- **DML (Data Manipulation Language)**: Insert, update, delete data
- **DQL (Data Query Language)**: Query and traverse graphs

## Authentication

Connect to ByoriDB with username and password:

```
Username: root
Password: value of BYORIDB_ROOT_PASSWORD, or the generated password logged at startup
```

## Quick Reference

| Category | Statements |
|----------|------------|
| DDL | `CREATE SPACE`, `DROP SPACE`, `CREATE TAG`, `ALTER TAG`, `DROP TAG`, `CREATE EDGE`, `ALTER EDGE`, `DROP EDGE` |
| DML | `INSERT VERTEX`, `UPDATE VERTEX`, `DELETE VERTEX`, `INSERT EDGE`, `DELETE EDGE` |
| DQL | `FETCH PROP`, `GO`, `MATCH`, `LOOKUP`, `FIND PATH` |

## Data Types

| Type | Description | Example |
|------|-------------|---------|
| `BOOL` | Boolean | `true`, `false` |
| `INT8` | 8-bit integer | `127` |
| `INT16` | 16-bit integer | `32767` |
| `INT32` | 32-bit integer | `2147483647` |
| `INT64` | 64-bit integer | `42` |
| `FLOAT` | 32-bit floating point | `3.14` |
| `DOUBLE` | 64-bit floating point | `3.14159` |
| `STRING` | Variable-length text | `'hello'` |
| `TIMESTAMP` | Unix timestamp | `1234567890` |
| `DATE` | Date | `2024-01-15` |
| `DATETIME` | Date and time | `2024-01-15T10:30:00` |

> **Note:** Use `INT64` instead of `INT` for integer types.

## Notes

1. **Case Sensitivity**: Keywords are case-insensitive (`CREATE` = `create`), but identifiers are case-sensitive.
2. **String Quotes**: Use single quotes for string values: `'hello'`
3. **Semicolons**: Optional at the end of statements.
4. **Vertex IDs**: Must be integers (`INT64`).
