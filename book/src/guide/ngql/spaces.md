# Space Management

A **Space** is a logical container for graph data (similar to a database in RDBMS).

## CREATE SPACE

```sql
CREATE SPACE <space_name> (vid_type = INT64);
CREATE SPACE my_graph (vid_type = INT64);
CREATE SPACE IF NOT EXISTS my_graph (vid_type = INT64);
```

**Parameters:**
- `vid_type`: Vertex ID type. Currently supports `INT64`.

## USE SPACE

```sql
USE <space_name>;
USE my_graph;
```

## SHOW SPACES

```sql
SHOW SPACES;
```

## DROP SPACE

```sql
DROP SPACE <space_name>;
DROP SPACE my_graph;
```
