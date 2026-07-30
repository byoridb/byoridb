# Backup and restore

[한국어](../ko/operations/backup.html)

ByoriDB ships one snapshot backup tool: `byoridb-backup`. It copies the redb
current-view and history tables into a separate `data.redb` file and records
metadata beside it.

The tool does **not** implement incremental backup, WAL archiving, point-in-time
recovery, per-space restore, S3/object-store output, or encryption. Arrange
off-site copying and encryption with external tooling after a snapshot succeeds.

## Build the tool

```bash
cargo build --locked --release -p byoridb --bin byoridb-backup
export PATH="$PWD/target/release:$PATH"
```

The database argument is a directory containing `data.redb`, not the redb file
itself. The standalone default is `data/storage` unless
`BYORIDB__STORAGE__DATA_PATHS` changes it.

## Backups require exclusive access

The standalone server and the backup CLI are separate processes. redb prevents
the CLI from opening a database that the server already has open, so a live
backup currently fails with `Database already open. Cannot acquire lock.`

Use a coordinated offline window:

1. stop traffic and stop `byoridb-server` gracefully;
2. confirm the server exited, allowing its final redb checkpoint to complete;
3. run `byoridb-backup create` against the data directory;
4. inspect the new backup and perform the checks below;
5. restart the server.

Do not copy a changing `data.redb` with `cp`. The backup implementation opens a
read transaction and copies both `kv` and `history` into a newly created redb
file.

## Create and inspect a snapshot

```bash
byoridb-backup create \
  --db /var/lib/byoridb/data \
  --backup-dir /var/lib/byoridb/backups \
  --label "daily-before-upgrade"
```

The command creates a timestamp-based directory such as:

```text
/var/lib/byoridb/backups/backup_1785313593/
├── backup_metadata.json
└── data.redb
```

On Unix, a newly created backup root is set to mode `0700`. Files contain raw
database data, including password hashes and graph properties, so retain
restrictive ownership after copying them elsewhere.

List and inspect catalog entries:

```bash
byoridb-backup list --backup-dir /var/lib/byoridb/backups
byoridb-backup list --backup-dir /var/lib/byoridb/backups --format json
byoridb-backup info \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

`--no-flush` remains in the CLI for compatibility, but the redb implementation
does not use a separate WAL flush and the option is currently a no-op. It does
not make a live backup possible.

## Verification limits

```bash
byoridb-backup verify \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

The current `verify` command checks that a backup can be found in the catalog;
it does not walk every table, validate application records, compare row counts,
or execute queries. A failed `create` can also leave a timestamp-named
directory, so do not treat `verify` alone as proof of a usable snapshot.

For each important backup:

- require a successful `create` exit code;
- confirm `backup_metadata.json` and a non-empty `data.redb` exist;
- restore into a new directory;
- start an isolated server on that directory with non-production ports;
- authenticate and check representative current and `AS OF` queries.

## Restore

Stop any process using the destination first. Restore to a new directory when
possible:

```bash
byoridb-backup restore \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593 \
  --target /var/lib/byoridb/restored-data
```

Point `BYORIDB__STORAGE__DATA_PATHS` at the restored directory before starting
the verification server:

```bash
export BYORIDB_ROOT_PASSWORD='managed-secret-for-this-environment'
export BYORIDB__STORAGE__DATA_PATHS=/var/lib/byoridb/restored-data
byoridb-server
```

The root password is not restored from a user record; root is always defined by
`BYORIDB_ROOT_PASSWORD` at server startup. Durable non-root user records are
inside the database snapshot.

`restore --overwrite` recursively removes an existing target directory before
copying the snapshot. Verify the exact target and retain a rollback copy before
using it.

## Retention commands

Delete one known backup:

```bash
byoridb-backup delete \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

Keep only the five newest catalog entries:

```bash
byoridb-backup cleanup \
  --backup-dir /var/lib/byoridb/backups \
  --keep 5
```

Both commands ask for confirmation unless `--force` is supplied. Review the
list before automated cleanup, especially after any failed create attempt.

`scripts/backup.sh` wraps `create`, count-based cleanup, and listing. It does
not stop the server or arrange exclusive access; an operator must provide that
coordination before scheduling the script.

## Operational checklist

- Keep backups on a failure domain separate from the primary data volume.
- Encrypt snapshots at rest and in transit with external, audited tooling.
- Monitor command exit codes and the presence/size of expected files.
- Test a full restore and representative temporal queries regularly.
- Record an environment-specific RPO/RTO from measured backup and restore
  drills; ByoriDB does not publish a universal value.
- Preserve the application build/configuration needed to read the snapshot and
  test upgrades on a copy before replacing production data.
