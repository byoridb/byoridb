# Bulk load runbook (AKS)

Offline bulk import of large datasets via `byoridb-bulkloader`, which writes
directly into the redb store — bypassing nGQL/HTTP and assigning sequential
INT64 vids so the B-tree stays append-friendly (avoids the random-VID write
amplification that makes HTTP seqload slow at this scale).

> **Why a Job, not HTTP:** at ~240M elements (89M nodes + 151M edges) HTTP
> `INSERT` is slow and degrades as the B-tree grows (commit `39f726e`), and its
> hash-mapped random VIDs hurt query performance too. The loader is single-pass,
> sorted, and uses relaxed durability.

## One-time: image must contain the loader

The loader binary ships in the `byoridb-server` image as of the 2026-06-25
Dockerfile change (`--bin byoridb-server --bin byoridb-bulkloader`). **Redeploy
first** so the live `byoridb-server:<sha>` image has `/usr/local/bin/byoridb-bulkloader`.

## Steps

### 1. DDL via the server (server still running)

The loader only *reads* metadata — create the space/tags/edges first. For a full
re-import, drop the old space first.

```sql
DROP SPACE IF EXISTS nexprice;
CREATE SPACE nexprice (vid_type = INT64);
USE nexprice;

CREATE TAG sku(...);
CREATE TAG product(...);
CREATE TAG brand(...);
CREATE TAG category(...);
CREATE TAG channel(...);

-- Edge type MUST be `same_as` (underscore), NOT `sameAs` — the loader rejects
-- `sameAs` because that name triggers the engine's owl:sameAs union-find merge.
CREATE EDGE same_as();
CREATE EDGE sold_on();
CREATE EDGE in_category();
CREATE EDGE has_brand();
CREATE EDGE child_of();
```

Column names in the CSVs must match the loader flags: node id column `id`,
edge endpoint columns `src` / `dst` (override with `--id-column` /
`--src-column` / `--dst-column` in the Job if your headers differ). Every CSV
column is preserved as a property; the id column also drives vid assignment.

### 2. Upload CSVs to the data PVC (`/app/data/import/`)

The data PVC is ReadWriteOnce, so upload while the server still holds it (a
helper pod can co-mount it on the same node) or during a short scale-down.
For 8.6GB, prefer Azure Blob + `azcopy` over `kubectl cp` (the latter is slow
and flaky at multi-GB):

```bash
# Helper pod sharing the data PVC, then azcopy from Blob into /app/data/import.
# (Server must be scaled to 0 first if you mount RWO from a separate pod.)
```

### 3. Scale the server down (release the RWO PVC)

```bash
kubectl -n byoridb scale statefulset byoridb-server --replicas=0
kubectl -n byoridb rollout status statefulset byoridb-server --timeout=120s
```

### 4. Pin the image tag and run the Job

```bash
SHA=$(kubectl -n byoridb get statefulset byoridb-server \
  -o jsonpath='{.spec.template.spec.containers[0].image}' | sed 's/.*://')
# (capture the sha BEFORE scaling down if the field clears; or read from ACR / the deploy log)

sed "s/PLACEHOLDER_SHA/$SHA/" deploy/azure/jobs/bulkload-nexprice.yaml \
  | kubectl apply -f -

kubectl -n byoridb logs -f job/byoridb-bulkload-nexprice
```

The loader logs per-file progress and a final summary (vertices / tagvid_entries
/ edges / duplicate_ids / dangling_edges). `dangling_edges > 0` means some edge
endpoint id was not among the loaded nodes — investigate the CSVs.

### 5. Scale the server back up

```bash
kubectl -n byoridb scale statefulset byoridb-server --replicas=1
kubectl -n byoridb rollout status statefulset byoridb-server --timeout=180s
```

### 6. Rebuild text indexes after direct-KV loads

The bulk loader and any future direct-KV import/update tool bypass the executor
DML hooks that maintain text-search indexes on `INSERT` / `UPDATE` / `DELETE`.
After loading or directly mutating searchable vertices, either:

- run the matching rebuild before serving search traffic, or
- implement the same text-index maintenance contract as the executor
  (`manifest`, `stats`, `doc`, and `post` keys).

For the nexprice product-name search index:

```sql
USE nexprice;
REBUILD TEXT INDEX ON product(prod_name);
```

Once this rebuild has run, normal nGQL DML keeps the index current
incrementally. Direct-KV writers must continue to maintain or rebuild it.

### 7. Verify with real queries

```sql
USE nexprice;
MATCH (n:sku) RETURN count(n);
MATCH (n:product) RETURN count(n);
SEARCH product.prod_name FOR 'NS84S03B_PAP' LIMIT 20;
GO FROM <some_product_vid> OVER same_as YIELD dst(edge);
GO FROM <some_sku_vid> OVER same_as REVERSELY YIELD dst(edge);   -- reverse index
```

## Notes

- **Memory:** the id map (original-id → vid) is held in RAM for the whole load:
  ~89M entries ≈ 6–8GB. The Job requests 16Gi / limits 32Gi.
- **Durability:** the Job uses `--durability relaxed`. A crash mid-load loses the
  last ≤64 commits; just re-run (vid assignment is deterministic per run, and a
  full re-import is idempotent if the space is dropped+recreated first).
- **Steady state:** the StatefulSet currently sets `BYORIDB_DURABILITY=none`
  (bulk-load mode). Per the comment in `03-statefulset.yaml`, remove that env var
  after loading so serving uses Immediate (fsync) durability.
- **`--verify`** is intentionally omitted from the Job: it scans the whole
  keyspace. Trust the loader's tallies; spot-check with the queries above.
- **Text search:** direct-KV loads do not update text-search postings. Rebuild
  `product(prod_name)` after each load, or add explicit text-index maintenance
  to the loader/tool before relying on `SEARCH`.
