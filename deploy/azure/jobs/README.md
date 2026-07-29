# Offline bulk-load runbook for AKS

> **English** | [한국어](README.ko.md)

This directory contains manually applied Kubernetes Jobs. Files here are not
part of the normal manifest apply loop under `deploy/azure/k8s/`.

`bulkload-nexprice.yaml` is a **dataset- and environment-specific example**. It
contains a namespace, PVC name, registry path, node selector, resource sizing,
schema names, and CSV paths that must be reviewed before every run. The presence
of the file does not imply that an AKS cluster is currently running.

## Safety model

`byoridb-bulkloader` opens the same redb database as the server and writes keys
directly. It bypasses HTTP, gRPC, sessions, and nGQL validation.

- Stop every process that can open the target redb database before starting the
  loader. The Kubernetes `ReadWriteOnce` access mode alone does not enforce this
  process-level exclusivity.
- Scaling the StatefulSet to zero causes downtime. Obtain operator approval and
  a tested backup or volume snapshot first.
- Never delete or replace a PVC as part of this runbook.
- Use `--durability=relaxed` only for reproducible imports. A crash can lose
  recent batches. Steady-state serving should use the server default,
  immediate durability.
- The example is lenient: duplicate node IDs and dangling edges are counted and
  skipped. Add `--strict` when either condition must abort the import.

## What the loader does

The loader:

1. Reads existing space, tag, and edge schemas from redb.
2. Assigns sequential `INT64` VIDs while loading node CSV files.
3. Persists the original ID-to-VID mapping.
4. Loads edge CSV files after all node files and writes forward/reverse edge
   entries and degree counters.
5. Preserves CSV columns as properties, converting values when a declared
   schema type is available.

It supports plain CSV and gzip-compressed CSV files. The default columns are
`id`, `src`, and `dst`; override them with `--id-column`, `--src-column`, and
`--dst-column`.

The exact edge type `sameAs` is reserved for the engine's irreversible
`owl:sameAs` canonical merge. The sample dataset therefore uses `same_as`.

## Preflight

Run all commands from the repository root. The examples assume namespace
`byoridb` and StatefulSet `byoridb-server`; change them for your environment.

1. Confirm the maintenance window and backup/restore procedure.
2. Review the Job before applying it:

   ```bash
   kubectl apply --dry-run=client -f deploy/azure/jobs/bulkload-nexprice.yaml
   kubectl -n byoridb get statefulset byoridb-server
   kubectl -n byoridb get pvc data-byoridb-server-0
   ```

3. Capture the exact deployed image before scaling down:

   ```bash
   DEPLOYED_IMAGE=$(kubectl -n byoridb get statefulset byoridb-server \
     -o jsonpath='{.spec.template.spec.containers[0].image}')
   test -n "$DEPLOYED_IMAGE"
   echo "$DEPLOYED_IMAGE"
   ```

4. Confirm that image contains `/usr/local/bin/byoridb-bulkloader`. Image
   contents, not a dated documentation claim, are the source of truth.
5. Upload CSV files to the paths referenced by the Job. For large files, use a
   storage-native transfer method and compare checksums after upload.

## Prepare schemas

The loader reads metadata but does not create schemas. While the server is still
running, create the target space and every tag/edge referenced by the Job.

The sample expects an `INT64` space and these names:

```ngql
CREATE SPACE IF NOT EXISTS nexprice (vid_type = INT64);
USE nexprice;

CREATE TAG IF NOT EXISTS sku(...);
CREATE TAG IF NOT EXISTS product(...);
CREATE TAG IF NOT EXISTS brand(...);
CREATE TAG IF NOT EXISTS category(...);
CREATE TAG IF NOT EXISTS channel(...);

CREATE EDGE IF NOT EXISTS same_as();
CREATE EDGE IF NOT EXISTS sold_on();
CREATE EDGE IF NOT EXISTS in_category();
CREATE EDGE IF NOT EXISTS has_brand();
CREATE EDGE IF NOT EXISTS child_of();
```

Replace `...` with real property definitions that match the CSV headers and
types. Do not copy the placeholders into a production query.

Dropping an existing space is destructive and is intentionally not included in
this runbook. If a clean re-import requires it, make that a separately reviewed
operation with a verified backup.

## Run the Job

1. Stop the server and wait until no server pod remains:

   ```bash
   kubectl -n byoridb scale statefulset byoridb-server --replicas=0
   kubectl -n byoridb rollout status statefulset/byoridb-server --timeout=180s
   kubectl -n byoridb get pods -l app.kubernetes.io/name=byoridb
   ```

2. Render the Job with the captured server image without editing the checked-in
   manifest, then apply it:

   ```bash
   kubectl set image -f deploy/azure/jobs/bulkload-nexprice.yaml \
     bulkloader="$DEPLOYED_IMAGE" --local -o yaml \
     | kubectl apply -f -
   ```

   Kubernetes Job pod templates are immutable. If a previous Job with this name
   exists, inspect its status and logs first. Delete only that Job object after
   explicit confirmation, then apply the new manifest.

3. Follow progress and inspect the terminal condition:

   ```bash
   kubectl -n byoridb logs -f job/byoridb-bulkload-nexprice
   kubectl -n byoridb wait --for=condition=complete \
     job/byoridb-bulkload-nexprice --timeout=24h
   kubectl -n byoridb get job/byoridb-bulkload-nexprice -o wide
   ```

The final summary reports vertices, tag-to-VID entries, edges, duplicate IDs,
and dangling edges. Treat nonzero duplicate or dangling counts as data-quality
signals, even in lenient mode.

The sample omits `--verify` because that option scans the full keyspace. Enable
it for small or medium imports, or run targeted queries after a large import.

## Return to service

Do not start the server until the Job pod has exited and no process holds the
redb database.

```bash
kubectl -n byoridb scale statefulset byoridb-server --replicas=1
kubectl -n byoridb rollout status statefulset/byoridb-server --timeout=600s
kubectl -n byoridb port-forward statefulset/byoridb-server 19669:19669
```

In another shell:

```bash
curl --fail http://127.0.0.1:19669/health
curl --fail http://127.0.0.1:19669/ready
```

Then authenticate and run dataset-specific counts and a small sample of forward
and reverse traversals. Compare them with independent source-file tallies; do
not rely only on the loader log.

## Failure handling

- If the Job fails, leave the server stopped until the loader pod has terminated.
- Preserve logs, Job YAML, image digest, CSV checksums, and final counters.
- A relaxed-durability partial import may need to be rerun or restored from the
  preflight snapshot. Choose based on the dataset's documented idempotency.
- If redb cannot reopen, do not delete the PVC. Escalate to the tested recovery
  procedure and work from a copy or snapshot whenever possible.
