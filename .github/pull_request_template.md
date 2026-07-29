<!-- Default template: English. Korean: pull_request_template.ko.md -->

## Summary

<!-- What problem does this PR solve, and why is this change needed? -->

## Changes

<!-- List the important implementation and documentation changes. -->

-

## Change type

- [ ] Bug fix
- [ ] New feature
- [ ] Breaking or migration-requiring change
- [ ] Refactor with no intended behavior change
- [ ] Documentation only
- [ ] Build, CI, deployment, or operations
- [ ] Security hardening

## Validation

<!-- List exact commands and results. Explain any check that was not run. -->

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
```

Additional targeted tests:

-

## Risk, compatibility, and rollout

<!-- Note API/storage/query compatibility, migration, deployment, rollback, and data risks. -->

- Compatibility impact:
- Rollout or migration:
- Rollback:

## Checklist

- [ ] The change is focused and follows the repository's code conventions.
- [ ] New or changed behavior has focused positive and negative tests.
- [ ] Production code contains no new `unwrap()`, `expect()`, `println!`, `eprintln!`, or `dbg!`.
- [ ] New shared dependencies are declared in root `[workspace.dependencies]` with rationale.
- [ ] English canonical documentation and its Korean mirror are both updated where needed.
- [ ] Incomplete or experimental behavior is labeled accurately.
- [ ] No credentials, `.env` files, private data, generated databases, or raw session IDs are included.
- [ ] Security-sensitive details were reported privately rather than placed in this public PR.

## Related issues

<!-- Example: Fixes #123 -->
