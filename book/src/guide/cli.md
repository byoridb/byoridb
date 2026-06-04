# CLI Usage

The ByoriDB CLI provides an interactive shell for executing queries.

## Starting the CLI

```bash
# Connect to local server
BYORIDB_USER=root BYORIDB_PASSWORD='<root-password>' byoridb-cli

# Connect to remote server
byoridb-cli --addr 192.168.1.100:9669

# With authentication
byoridb-cli --user root --password mypassword
```

## CLI Commands

### Connection Commands

| Command | Description |
|---------|-------------|
| `:help` | Show help message |
| `:quit` or `:exit` | Exit the CLI |
| `:clear` | Clear the screen |

### Execution

Type nGQL statements and press Enter to execute:

```
(root@localhost:9669) > CREATE SPACE test(vid_type=INT64);
Execution succeeded

(root@localhost:9669) > USE test;
Switched to space `test`

(root@localhost:9669) [test] > SHOW TAGS;
Empty set
```

### Multi-line Statements

For long queries, end lines with `\` to continue:

```
(root@localhost:9669) [test] > INSERT VERTEX person(name, age) VALUES \
                              > 1:('Alice', 30), \
                              > 2:('Bob', 25);
Execution succeeded
```

### Query Results

Results are displayed in a table format:

```
(root@localhost:9669) [test] > FETCH PROP ON person 1;
+----+--------+-----+
| id | name   | age |
+----+--------+-----+
| 1  | Alice  | 30  |
+----+--------+-----+
1 row in set
```

## Session Management

### Current Space

The current space is shown in the prompt:

```
(root@localhost:9669) [my_space] >
```

### Switch Space

```sql
USE another_space;
```

## Tips

1. **Tab Completion**: Press Tab for keyword completion
2. **History**: Use Up/Down arrows to navigate command history
3. **Semicolons**: Optional at end of statements
4. **Comments**: Use `--` for single-line comments

```sql
-- This is a comment
CREATE TAG person(name STRING);  -- inline comment
```

## Non-Interactive Mode

Execute queries from command line:

```bash
# Single query
byoridb-cli --execute "SHOW SPACES;"

# From file
while IFS= read -r query; do
  byoridb-cli --execute "$query"
done < queries.ngql

# With connection options
byoridb-cli --addr remote.server:9669 --execute "SHOW SPACES;"
```

## Output

```bash
# Results are printed as tables in interactive mode.
byoridb-cli
```

## Error Messages

```
(root@localhost:9669) [test] > SELECT * FROM person;
Error: Syntax error near 'SELECT'

(root@localhost:9669) > USE nonexistent;
Error: Space 'nonexistent' not found
```
