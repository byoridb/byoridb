[한국어](../ko/guide/cli.html)

# Command-line client

`byoridb-cli` is a small authenticated gRPC client. It supports a one-line REPL
and a single-query non-interactive mode; it is not a full administrative shell.

## Connect

Credentials are required. Prefer environment variables so the password does not
appear in the command line or shell history:

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='your-root-password'
cargo run --release -p byoridb-client --bin byoridb-cli
```

For another endpoint:

```bash
byoridb-cli --addr 192.0.2.10:9669
```

The current transport is plaintext gRPC. Use a private network, VPN, or a
trusted TLS tunnel for remote connections.

## Options

| Option | Meaning |
| --- | --- |
| `-a, --addr <ADDR>` | gRPC address; default `127.0.0.1:9669` |
| `-u, --user <USER>` | Required username; also `BYORIDB_USER` |
| `-p, --password <PASSWORD>` | Required password; also `BYORIDB_PASSWORD` |
| `-e, --execute <QUERY>` | Execute one query and exit |
| `-h, --help` | Show generated help |
| `-V, --version` | Show the client version |

Passing `--password` exposes the value to shell history and may expose it to
local process inspection. `BYORIDB_PASSWORD` is the safer built-in option.

## REPL behavior

After authentication, the prompt is:

```text
Connected to byoridb-server at 127.0.0.1:9669
byoridb>
```

Enter one logical request per line:

```sql
CREATE SPACE demo;
USE demo;
SHOW TAGS;
```

The REPL provides line editing and up/down history through `rustyline`. It saves
accepted lines to `history.txt` in the current working directory when it exits.
That file can contain sensitive query text, including passwords in `CREATE USER`
or `ALTER USER` statements. Protect or remove it as appropriate.

The only built-in exit words are `quit` and `exit` (case-insensitive). Ctrl-C
and Ctrl-D also close the REPL. Colon commands such as `:help`, multiline
continuations, and tab completion are not implemented. Put a compound request
on one line if it contains semicolon-separated clauses.

## Results

Datasets are rendered as Unicode tables. A data query with columns but no rows
prints `Empty set.`; a successful statement without a dataset prints
`Executed successfully.`. Other JSON values use pretty-printed JSON as a
fallback.

## Execute one query

```bash
BYORIDB_USER=root \
BYORIDB_PASSWORD='your-root-password' \
byoridb-cli --execute 'SHOW SPACES;'
```

The command exits with a non-zero status if connection, authentication, query
execution, or JSON decoding fails.

For scripts, prefer the structured client library or HTTP API when you need to
handle multiple statements and results reliably. The CLI has no native
`--file` option.
