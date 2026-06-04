# Schema Definition

Define the structure of your graph with Tags (vertex types) and Edges.

## Tags (Vertex Types)

### CREATE TAG

```sql
CREATE TAG <tag_name> (
    <property_name> <data_type> [NULL | NOT NULL] [DEFAULT <value>],
    ...
);
```

**Examples:**

```sql
CREATE TAG person(name STRING, age INT64);
CREATE TAG player(name STRING NOT NULL, score INT64 DEFAULT 0);
CREATE TAG product(
    name STRING,
    price DOUBLE,
    in_stock BOOL DEFAULT true,
    created_at TIMESTAMP
);
```

### ALTER TAG

Add new properties to existing tags (online schema change):

```sql
ALTER TAG <tag_name> ADD (<property_name> <data_type> [NULL | DEFAULT <value>]);
```

**Examples:**

```sql
-- Add nullable column
ALTER TAG person ADD (email STRING NULL);

-- Add column with default value
ALTER TAG player ADD (level INT64 DEFAULT 1);
```

> **Note:** New columns must be either nullable (`NULL`) or have a default value. Existing vertices will return `NULL` or the default value for the new property.

### SHOW TAGS

```sql
SHOW TAGS;
```

### DESCRIBE TAG

```sql
DESCRIBE TAG person;
DESC TAG person;
```

### DROP TAG

```sql
DROP TAG <tag_name>;
DROP TAG person;
DROP TAG IF EXISTS person;
```

## Edge Types

### CREATE EDGE

```sql
CREATE EDGE <edge_name> (
    <property_name> <data_type> [NULL | NOT NULL] [DEFAULT <value>],
    ...
);
```

**Examples:**

```sql
CREATE EDGE knows();
CREATE EDGE follow(since TIMESTAMP);
CREATE EDGE purchase(
    quantity INT64 DEFAULT 1,
    price DOUBLE,
    purchased_at DATETIME
);
```

### ALTER EDGE

Add new properties to existing edge types:

```sql
ALTER EDGE <edge_name> ADD (<property_name> <data_type> [NULL | DEFAULT <value>]);
```

**Examples:**

```sql
ALTER EDGE follow ADD (weight DOUBLE NULL);
ALTER EDGE purchase ADD (discount DOUBLE DEFAULT 0.0);
```

### SHOW EDGES

```sql
SHOW EDGES;
```

### DESCRIBE EDGE

```sql
DESCRIBE EDGE knows;
DESC EDGE follow;
```

### DROP EDGE

```sql
DROP EDGE <edge_name>;
DROP EDGE knows;
DROP EDGE IF EXISTS knows;
```

## Indexes

### CREATE INDEX

Create index for faster lookups:

```sql
CREATE TAG INDEX <index_name> ON <tag_name>(<property_name>);
CREATE EDGE INDEX <index_name> ON <edge_name>(<property_name>);
```

**Examples:**

```sql
CREATE TAG INDEX person_name_idx ON person(name);
CREATE EDGE INDEX follow_since_idx ON follow(since);
```

### SHOW INDEXES

```sql
SHOW TAG INDEXES;
SHOW EDGE INDEXES;
```

### DROP INDEX

```sql
DROP TAG INDEX <index_name>;
DROP EDGE INDEX <index_name>;
```

## Schema Best Practices

1. **Choose appropriate data types** - Use `INT64` for IDs, `STRING` for text, `DOUBLE` for decimals
2. **Use defaults wisely** - Set sensible defaults to simplify inserts
3. **Plan for schema evolution** - Use nullable columns for optional data
4. **Create indexes** - Index frequently queried properties for performance
