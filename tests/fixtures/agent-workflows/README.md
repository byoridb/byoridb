# Agent workflow contract fixtures

[한국어](README.ko.md)

These EVAL-1 fixtures exercise the first AGT-1 discovery contract and existing
query authorization. They are deterministic service tests, not model-generated
work or measurements of autonomous-agent success.

Run from the repository root:

```bash
cargo test --locked --test agent_workflow_test -- --test-threads=1
```

Each fixture declares its task, setup, expected typed answer, denied operations,
expected final graph, evidence scope, seed, and repetitions. Every run uses a
fresh in-memory database and separate root/GUEST sessions. Test credentials are
synthetic and never contact a deployment. No random data or model is involved.

| Fixture | Expected answer and final state |
|---|---|
| `read_int64.json` | Exactly Ada and Grace, retaining integer IDs 9007199254740993 and 9007199254740995 beyond JavaScript's exact-number range |
| `read_string.json` | Exactly Ada and Grace, retaining distinct external string IDs `42` and `사람:grace` |

The harness discovers the selected schema without altering session selection,
checks the global-role permission boundary and unsupported capabilities, reads
the expected graph, attempts prohibited writes through both request-level
`read_only` and the GUEST role, and verifies the exact final person rows. It then
signs out the GUEST session and verifies query/discovery access is revoked.
Integer expectations use decimal strings with explicit `int64` tags; string IDs
remain tagged `string`. These are fixture values, not a new query wire encoding.

Discovery metadata is not inference evidence. The answers in this task are
asserted rows; no claim is made about external source truth. This fixture does
not verify historical state, hidden metadata counters, or every graph type.
Storage-fault, malformed-metadata, cap, and HTTP/gRPC parity regressions live with
the graph/executor implementations.

Durable timeout retries, revision-checked concurrent writes, and snapshot
continuation remain unsupported and are recorded as such. EVAL-1 remains in
progress until inference/retraction and those implemented future contracts have
fixtures and a versioned model-run protocol. A later model evaluation must
record the model/version/date, prompts/settings, dataset version, repetitions,
and call/token/time budgets before execution. Compare cost only among runs that
pass the same answer, evidence, and final-state checks. No latency/token savings
or autonomous-agent success rate is claimed by these two fixtures.
