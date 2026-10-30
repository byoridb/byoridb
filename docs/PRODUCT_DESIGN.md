# ByoriDB product philosophy and design

> **English** | [한국어](PRODUCT_DESIGN.ko.md)
>
> Adopted direction: 2026-10-30. This document defines the product and architecture
> target. It is not an announcement of implemented APIs or a released capability.

ByoriDB is an **agent-first, general-purpose graph and ontology database**.
It gives agents a place to query relationships, apply explicit rules, inspect
derivations, and change shared state under enforceable contracts. The product
aims to improve correct task completion and state consistency within a bounded
execution budget. These are evaluation goals, not measured competitive claims.

This is the authoritative product and interaction design. [PLAN.md](PLAN.md)
owns delivery order, task status, acceptance evidence, and release prerequisites.
The released baseline is v0.4.20; subsequent implementation status is identified there.
Update both language editions when changing a contract or product decision.

## Product boundary

Agents are the primary API consumers. Humans operate, authorize, inspect, audit,
and recover the database. A CLI and readable documentation remain necessary for
those responsibilities. Their role is to make the machine-facing contract
observable and operable rather than define a separate source of truth.

The database owns durable facts, relationships, schemas, queries, deterministic
rule execution, derivations, constraints, and data access enforcement. Applications
own model selection, prompts, task planning, approvals, business actions, and
external side effects. ByoriDB does not become an LLM runtime or an agent scheduler.

Agent memory is one application of the engine. Other workloads include dependency
analysis, changing domain models, policy evaluation, and shared operational state.
The graph and ontology core remains general purpose. A protocol adapter does not
define the storage model or become the only way to use the database.

Keep the Rust single-node engine and redb storage foundation. HTTP/gRPC, supported
clients, and future adapters must share execution and authorization semantics.
Distributed operation, RDF/SPARQL compatibility, and alternative execution engines
retain the entry gates in PLAN.md; this direction does not approve a rewrite.

## Design principles

1. **Discover before guessing.** An agent can inspect authorized schema, value
   types, supported operations, limitations, and contract versions.
2. **Make meaning machine-readable.** Stable typed values, errors, completeness,
   and write outcomes must not require interpretation of natural-language text.
3. **Separate fact, derivation, and claim.** Assertions and logical consequences
   carry different semantics; model-generated claims are not automatically true.
4. **Bound work without hiding loss.** Budgets and cancellation have explicit
   behavior. Truncated evidence and interrupted queries cannot masquerade as complete.
5. **Make retries and concurrent changes explicit.** Durable outcomes, idempotency,
   and revision checks protect shared state rather than relying on agent memory.
6. **Enforce authority at the server.** Prompts, tool descriptions, client hints,
   and a successful preview do not grant permissions.
7. **Measure completed work.** Evaluate the final answer and database state before
   claiming savings in tool calls, tokens, time, or infrastructure.

## Intended agent interaction

The target interaction is:

**Discover → query → inspect evidence → prepare change → apply with authority → verify.**

| Step | Agent receives or provides | Required server behavior |
|---|---|---|
| Discover | Authorized schema, capability version, limits, value/ID types | Filter metadata by current rights; expose supported and unsupported behavior |
| Query | Typed operation or parameterized query, read scope, budget | Enforce rights and limits; identify the data view and result completeness |
| Inspect evidence | Selected facts, derivation references, source references | Preserve their distinct meanings and offer bounded continuation |
| Prepare change | Intended mutation, expected revisions, impact request | Validate and preview within a declared scope; do not mutate or grant authority |
| Apply | Mutation or valid prepared plan, preconditions, idempotency key | Recheck permissions and revisions at commit; return an explicit outcome |
| Verify | Outcome lookup and authorized state read | Resolve uncertain outcomes and compare committed state to the intended change |

Read-only use stops after evidence inspection. Preparation does not require a
human confirmation for every mutation: the caller's application chooses its
approval policy. The database enforces the permissions and preconditions needed
for any apply, whether the initiator is an agent, a person, or an automated job.

The operation labels and fields discussed below are **proposed contracts**.
They are not executable endpoints, current SDK methods, or an existing MCP server.
Exact transport names and wire schemas are settled by their implementation tasks.

## Machine-facing contract

### Discovery and values — AGT-1, API-1, API-2

Discovery must report the selected space, schema version, visible tags/edge types,
property types, external vertex-ID type, supported operations, and relevant limits.
Report permissions scoped to the authenticated principal and space. A capability
response describes current availability; it is not a transferable authorization
token, and subsequent calls still undergo authorization.

Responses must not disclose inaccessible schema, space names, counts, examples,
or capabilities. Unsupported and unavailable-to-this-principal behavior need
consistent disclosure rules rather than authorization leaks in error messages.

Represent INT64 IDs, revisions, timestamps, and counters without JSON number
precision loss. Define tagged or string-backed encodings and round-trip tests
across JSON, protobuf, and SDKs. Preserve the difference between string `"42"`
and integer `42`; expose external IDs rather than internal string-ID mappings.
Null, absent properties, empty collections, and typed complex values remain distinct.

Add typed parameter binding and structured operations through shared validation
and execution. Never implement an adapter by interpolating input into query text.
Retain the text query API with a documented compatibility policy; advanced callers
can use it without bypassing the same authorization and result contracts.

### Results, evidence, and uncertainty — AGT-2

A result envelope needs a contract version, request reference, selected space,
data-view reference, typed data, execution status, and completeness information.
Use ordered column descriptors and positional rows, or an equally lossless shape,
so duplicate column names cannot overwrite values. Define equivalent JSON and
protobuf representations for every supported value, including non-finite numbers.
Keep query-result completeness separate from evidence-presentation completeness:
a complete answer can have a bounded explanation, but that bound must be visible.

User-specified `LIMIT` defines the query result. A hidden scan cap, timeout, or
failed partition does not. The default remains a complete query-defined result
or an explicit error. A future partial-result mode requires explicit opt-in,
a machine-readable reason, and a prohibition on treating it as a complete answer.

Bound evidence by items, bytes, traversal/derivation depth, and execution time.
Return selected premise/rule references and a continuation mechanism rather than
an unbounded closure. State which limits were reached and what was not evaluated.
Deduplicate references without deleting alternative derivations silently.

Cursors must bind to principal, space, operation/parameters, ordering, and data
view; be opaque or integrity-protected; and have an explicit lifetime. Define
snapshot retention and behavior under writes, revocation, restart, and expiration.
Do not silently continue against a new view. Until snapshot continuation exists,
reject that capability rather than promise a stable paginated snapshot.

Use separate semantic states for a matching assertion, a derived fact, explicit
negation where the model supports it, and unknown or unevaluated propositions.
No matching row only establishes no match in the declared view and query scope.
It is not universal falsehood. An incomplete evaluation cannot establish absence.
Define the relevant open/closed-world assumption per supported operation.

### Assertion origin and rule eligibility — SEM-8

Logical provenance records which facts and rules derive a result. Source lineage
records where an assertion came from and how it was ingested or changed. A proof
of derivability does not establish that the input facts describe the real world.
LLM-generated claims require explicit origin and epistemic status; storing a
claim as data must not silently promote it to a trusted premise in every rule set.

SEM-8 defines the source/ingestion metadata and rule-eligibility policy, including
missing attribution and migration of existing assertions. Enforce the same policy
through structured, text, and bulk-write paths and preserve it through recovery.

### Errors, retries, and write outcomes — API-2, AGT-3

Return stable error categories and structured context: invalid request, unsupported
capability, unauthenticated, unauthorized, conflict, resource exhaustion, expired
view/plan, internal/storage failure, and outcome uncertainty. Natural-language
messages help operators but are not the retry protocol.

Distinguish whether an error is retryable from whether a mutation was applied.
For writes, report known-not-applied, committed, partially-applied, or unknown as
applicable. Compound execution can have a committed prefix until atomic batches
are implemented. Include the failing statement and committed prefix when known;
never label a general execution error as rollback.

An idempotency record binds a key to authenticated principal, space, operation,
canonical request hash, and durable outcome. Concurrent duplicate requests must
coordinate on the same record. The same key with a different payload is rejected.
A repeated completed request returns the recorded outcome subject to current
access checks, without repeating the mutation.

The supported transaction boundary must atomically couple state change and outcome
recording, including supported history, index, counter, and inference effects.
Atomic KV batches alone do not establish this whole-operation guarantee.
For unsupported operations, reject idempotency before mutation rather
than imply protection. Recovery must reconcile in-progress records after a crash.
Define canonicalization, maximum key/payload size, quotas, retention duration,
expiry behavior, and the exact deduplication guarantee before shipping AGT-3.
Do not promise unbounded or cross-system exactly-once execution.

A transport timeout or disconnect may leave the caller uncertain even if the
server committed. The caller checks durable outcome using the original operation
reference/key before deciding to retry. Cancellation specifies where work can stop
and which commits may already have occurred; it does not undo committed writes.

### Concurrent mutation and preparation — AGT-4

Mutations support expected data revisions and schema versions with a defined
scope: the implementation must choose and document space-wide or entity-level
conflict detection. A stale precondition fails with a conflict and no mutation
within the supported atomic boundary. Reject unsupported preconditions explicitly.

A prepared change describes intended effects, affected scope, assumptions,
expected revisions, schema version, and expiry. The server binds it to principal,
space, request, and plan version. A preview is neither a reservation nor an
authorization grant. Preview/apply must not hide a race between validation and commit.

Apply revalidates permission, schema, revisions, and plan lifetime at the commit
boundary. Expired or stale plans must be recomputed and, where application policy
requires it, approved again. Never silently apply a different plan under an old
approval. Application-side approvals and business-side-effect execution remain
outside the engine; their records may reference the durable database operation.

Current irreversible `sameAs` merges require explicit impact/conflict inspection
and a retained merge record before being offered as an agent-safe prepared action.
Preview does not create undo support. Splitting merged identities is a separate
data-preservation design, not the inverse of applying the preview.

## Authority, untrusted data, and operations

Enforce least privilege with durable space-scoped grants and consistent checks on
direct, compound, profiled, discovery, evidence, and administrative paths. A
read-only request restriction can reduce a writable session's authority for that
request, but cannot expand it. Revocation must affect subsequent execution and
must be rechecked before committing a prepared change.

Treat retrieved text, properties, source documents, and claim content as data,
including content that looks like tool instructions. Structured responses must
separate payload data from trusted protocol control fields. Tool annotations and
adapter prompts are usability hints, never enforcement. The engine cannot prevent
every model-level prompt injection; it must prevent data from granting DB rights.

Credentials, session tokens, and sensitive graph values must not leak through
discovery, traces, fixtures, metrics, or evaluation artifacts. Human operations
need authorized audit references, content-aware backup verification, restore
drills, storage-version policy, and a supported secure deployment profile.
The DEP-1 dependency audit remains a release-readiness requirement; PLAN records
the current automatic-release enforcement gap.

## Architecture and migration

| Layer | Responsibility under this design |
|---|---|
| `byoridb-graph` | Shared GraphService boundary, authenticated discovery/operations, request limits, protocol mapping, outcome handling |
| `byoridb-common` | Shared typed values and stable contract/error definitions; explicit wire conversion rules |
| `byoridb-parser` | Existing text syntax and typed parameter syntax; no independent authority decisions |
| `byoridb-executor` | Query completeness, rule/constraint semantics, evidence selection, supported mutation behavior |
| `byoridb-storage`, `byoridb-kvstore` | Atomic persistence boundaries, revision/outcome durability, history, backup and recovery |
| `byoridb-client` | Compatible typed transport bindings and client-facing outcomes; server remains authoritative |
| `byoridb-bulkloader` | Import parity or explicit rebuild/validation mode; no implicit semantic bypass |
| External adapters, including `byori` where applicable | Map protocol tools to supported contracts; preserve rights, types, limits, outcomes and compatibility |

This is a responsibility map, not a claim that the target contracts are implemented.
The existing HTTP/gRPC service, primitive protobuf result types, and request-level
`read_only` checks are useful foundations. `SHOW`/`DESCRIBE` already provide
query-based discovery; a dedicated versioned capability manifest is new work.
Complex protobuf values currently use JSON fallback. HTTP rows keyed by column
name can lose duplicate names, and JSON INT64 numbers can lose client precision.
Errors are coarse, and `execute_with_params` currently ignores its parameter map.
These are gaps, not complete agent-facing guarantees. `EXPLAIN` describes a query
plan, not a mutation preview; `PROFILE` executes its statement.

Current `WHY` uses integer IDs; history queries cover asserted vertex/edge `FETCH`
only. Historical inference closure is unsupported. Inference can encounter caps,
compound writes are not general transactions, and `sameAs` merging is irreversible.
Selected ontology features do not imply full OWL, SHACL, RDF, SPARQL, or nGQL
conformance. Preserve these boundaries in capability discovery and adapter docs.

Adopt contracts incrementally. Begin AGT-1 with read-only, authorized discovery
schemas and fixtures alongside QRY-2 completeness work. Bring API-1/API-2 and
EVAL-1 into Stage 1; build scoped authorization and secure deployment in Stage 2.
Durable writes enable AGT-3/AGT-4; evidence follows its semantic prerequisites.
SDKs and AGT-5 adapter conformance follow the core contract in Stage 4. PLAN.md
is authoritative if task sequencing changes; new transport shapes never bypass it.

## Agent evaluation — EVAL-1

Use fixed model/version, prompts, tool definitions, datasets/seeds, permissions,
budgets, attempt counts, and environment. Record nondeterminism and report repeated
trials rather than selecting a successful run. Include schema discovery, exact
relationship queries, explanations, deletion/retraction, retry after disconnect,
concurrent conflict, authorization denial, and bounded/incomplete-result handling.

Score exact final database state and answer provenance against an oracle. Include
unauthorized changes, duplicate writes, stale conclusions, invented evidence,
and hidden partial results as failures. A plausible answer is not task success.
Use synthetic or explicitly approved data; redact credentials and sensitive values
from prompts, traces, persisted results, and shared benchmark artifacts.

Only compare calls, input/output tokens, latency, and resource use after correctness
and authority checks. Report failed-attempt cost and retry cost as well as successful
task cost. Separate model cost, adapter overhead, and DB execution cost.
Compare graph databases with equivalent adapters and equivalent task semantics;
workflow frameworks are a separate comparison unless their underlying storage and
orchestration are held equivalent. Do not attribute an adapter advantage to the
storage engine or compare a weaker task to a stronger one.

## Decisions required before implementing each contract

- Versioning and exact wire schema, including lossless integer and complex values.
- Authorized discovery disclosure policy and the first structured operation set.
- Data-view/revision granularity, cursor retention, and invalidation after restart.
- Supported atomic mutation scope, idempotency retention/quotas, and crash recovery.
- Preview scope and impact bounds; how callers identify a durable operation.
- Assertion/claim trust policy and supported logical absence/negation semantics.
- First adopter SDK, protocol adapter ownership, and representative evaluation tasks.

Resolve these in the owning task before advertising its capability. The accepted
direction is firm; exact field names, retention values, and unfinished guarantees
remain design work with explicit acceptance tests.
