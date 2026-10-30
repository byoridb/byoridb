# Agent-first product design

**English** | [한국어](../ko/architecture/agent-first.html)

ByoriDB is designed for agents that need to understand relationships, inspect
derivations, and maintain consistent state. The engine remains a general-purpose
graph and ontology database. Agent memory, dependency analysis, and knowledge
catalogues are application profiles over it.

**Status: adopted product direction, with implementation delivered incrementally.**
The canonical [product design](https://github.com/byoridb/byoridb/blob/main/docs/PRODUCT_DESIGN.md)
defines the target contracts; the [roadmap](../development/roadmap.html) tracks
acceptance and delivery. The [API reference](../reference/api.html) documents
what can be called today. The flow below is a design, not an executable API.

## The primary interaction

```text
Discover -> Query -> Inspect evidence -> Prepare change -> Apply -> Verify
    ^          |             |                 |            |        |
    +----------+-------------+-----------------+------------+--------+
          explicit errors, bounded results, outcome recovery
```

Each step has a distinct responsibility:

| Step | Target behavior |
|---|---|
| Discover | Return only authorized schemas and available operations, with types, versions, limits, and examples |
| Query | Accept typed arguments and return bounded results with explicit completeness and consistency scope |
| Inspect evidence | Distinguish asserted data, derivation rules/premises, source attribution, and unknown evidence |
| Prepare change | Validate a proposed change and bind its preview to the request, scope, and relevant revisions |
| Apply | Recheck authority and preconditions, commit within the advertised atomic boundary, and record the operation outcome |
| Verify | Retrieve the recorded outcome after a lost response; reconcile without blindly issuing another mutation |

Agents handle task planning and choose actions. ByoriDB enforces types,
constraints, access, execution limits, and the advertised data guarantees.
Humans provision credentials, inspect evidence and changes, and recover the
service. A preview does not grant authority; a tool description is not an access
control mechanism.

## Evidence and uncertainty

An inference explanation shows how a conclusion follows from stored premises
and supported rules. It does not prove that an asserted fact is true in the
outside world. Source attribution and logical derivation must remain distinct.
An LLM-generated assertion must not acquire verified status merely by being
stored or cited.

An empty result describes a query over a specified scope; it is not universal
proof of absence. A limit, permission boundary, missing proof, or expired cursor
must not silently become a successful empty answer. Stored text and evidence
are data, not instructions or authority for the agent.

## A shared engine contract

The target structured surface and existing text-query path converge on the same
service authorization, execution, and storage guarantees. HTTP, gRPC, SDK, and
MCP adapters translate that contract. Adapters must not implement a second
reasoner or bypass permissions by calling storage directly.

[Byori](https://github.com/byoridb/byori) supplies an agent-memory application
and MCP integration. Its domain tools do not establish a universal engine
protocol. Adapter conformance is planned as AGT-5; this engine does not yet ship
the generic structured operation surface described here.

## What changes in development

- API-1/2, AGT-1/2, and EVAL-1 begin with query correctness: typed inputs,
  discoverability, explicit outcomes, bounded evidence, and agent evaluation.
- Recovery and atomic write work underpin durable retry outcomes and
  revision-bound changes (AGT-3/4). Access isolation and deployment security
  advance alongside them.
- Ontology consistency and explanation remain core work. Broad standards,
  historical inference, and distribution retain their separate entry gates.
- CLI and documentation support operators and debugging; successful autonomous
  workflows become the main product acceptance evidence.

## How the product will be evaluated

EVAL-1 has deterministic discovery/read/access-denial fixtures with exact
answer and final-state assertions; no autonomous model evaluation has run.
Remaining work includes bounded retrieval, derivation explanation, retraction, dropped-response retry,
concurrent changes, and access denial on known fixtures. Grade the answer and
the final database state, including unauthorized or duplicate writes.

Pin the model, prompt, tools, server revision, data, budgets, and run settings;
report repeated outcomes and all failures. Compare task success first, then
calls, tokens, latency, and operating cost. Query benchmarks remain useful but
do not establish agent task success by themselves.

## Current limits still apply

HTTP/gRPC expose query text and discovery contract 1: authenticated, bounded
tag/edge/property schemas, global-role permissions and explicit unsupported
capabilities. `read_only` is enforced. Typed external parameters, durable request
deduplication, and revision-bound preview/apply remain unimplemented.
`WHY` accepts integer IDs and returns tabular explanations. Asserted-fact
`FETCH ... AS OF` does not reconstruct historical inference. Compound
statements may leave committed earlier clauses, and `sameAs` merges are
irreversible. See the [runtime overview](overview.html) and
[API reference](../reference/api.html) before integrating.
