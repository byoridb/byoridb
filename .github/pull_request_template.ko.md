<!-- 한국어 template. 기본 영문: pull_request_template.md -->

## 요약

<!-- 이 PR이 해결하는 문제와 변경이 필요한 이유를 설명하세요. -->

## 변경사항

<!-- 중요한 구현 및 문서 변경을 나열하세요. -->

-

## 변경 유형

- [ ] Bug fix
- [ ] 새 기능
- [ ] Breaking change 또는 migration이 필요한 변경
- [ ] 의도한 동작 변경이 없는 refactor
- [ ] 문서만 변경
- [ ] Build, CI, deployment 또는 운영
- [ ] Security hardening

## 검증

<!-- 실행한 정확한 command와 결과를 적고, 실행하지 않은 검사가 있으면 설명하세요. -->

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
```

추가 targeted test:

-

## Risk, compatibility 및 rollout

<!-- API/storage/query compatibility, migration, deployment, rollback, data risk를 적으세요. -->

- Compatibility 영향:
- Rollout 또는 migration:
- Rollback:

## Checklist

- [ ] 변경 범위가 집중되어 있고 저장소 code convention을 따릅니다.
- [ ] 새 동작 또는 변경한 동작에 집중된 positive/negative test를 추가했습니다.
- [ ] Production code에 새 `unwrap()`, `expect()`, `println!`, `eprintln!`, `dbg!`가 없습니다.
- [ ] 새 공용 dependency를 사유와 함께 root `[workspace.dependencies]`에 선언했습니다.
- [ ] 필요한 영문 canonical 문서와 한국어 mirror를 모두 갱신했습니다.
- [ ] 미완성 또는 experimental behavior를 정확히 표시했습니다.
- [ ] Credential, `.env` 파일, private data, 생성된 database, raw session ID를 포함하지 않았습니다.
- [ ] Security-sensitive 세부사항은 이 public PR이 아니라 비공개로 제보했습니다.

## 관련 issue

<!-- 예: Fixes #123 -->
