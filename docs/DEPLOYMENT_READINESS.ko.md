# Standalone 릴리스 인수 기준

> **한국어** | [English](DEPLOYMENT_READINESS.md)
>
> **v0.4.23** 검증 릴리스, 게시 시각 2026-10-31 07:06:25 UTC.
> 로컬 검증·호스팅 gate·게시 artifact 근거를 아래에서 구분합니다.

1차 지원 배포는 standalone 서버 하나와 redb volume이며 HTTP와 gRPC가 인증,
session, 실행을 공유합니다. Agent는 권한을 검사한 discovery와 text query를 사용합니다.
Typed operation, idempotency, revision, SDK와 multi-node 후속 계획은
[PLAN.ko.md](PLAN.ko.md)에 남아 있습니다.

## 릴리스 인수 — v0.4.23

아래 standalone 계약에 대해 QRY-3, OPS-1, SEC-1은 **DONE (released v0.4.23)**입니다.
구현 commit
[`2cb4072d0314bc911e9165c218d87feeacadbc10`](https://github.com/byoridb/byoridb/commit/2cb4072d0314bc911e9165c218d87feeacadbc10)의
[CI](https://github.com/byoridb/byoridb/actions/runs/41177637017)가 성공했습니다.
버전 갱신·tag commit
[`fb96cb3751934f447833b7ce97e3230459be1830`](https://github.com/byoridb/byoridb/commit/fb96cb3751934f447833b7ce97e3230459be1830)은
[Auto-release](https://github.com/byoridb/byoridb/actions/runs/41177636969)를 통과했고
[게시된 v0.4.23 tag](https://github.com/byoridb/byoridb/releases/tag/v0.4.23)의 대상입니다.
더 넓은 완전성·결과 envelope 작업인 QRY-2는 IN PROGRESS를 유지합니다.

| 검증 | 확인한 결과 |
|---|---|
| Formatting, locked workspace check와 Clippy | 모든 target/feature에서 PASS; Clippy는 `-D warnings` 적용 |
| 직렬 locked workspace test | 로컬·호스팅 CI·정확한 ref의 release gate 모두 **37개 suite에서 1,272 passed, 0 failed, 기존 ignored 8개** |
| 의존성 audit | **취약점 0건**; 기존 `bincode` 1.3.3 unmaintained 경고는 저장 형식 migration 제약으로 추적 유지 |
| Docker TLS 배포 | Source·prebuilt image 검증 PASS, 최종 통합 source `--build` 검증 PASS. 실제 HTTPS/gRPC 인증, 비공개 backend, 운영 route 차단, 인증서 교체, 재시작 후 데이터 유지와 log 비노출 확인 |
| 독립 release/profile 검토 | 수정 후 차단 이슈 없음 |
| Native release build | **1분 43초**, PASS |
| 실제 native binary smoke | PASS: 인증된 HTTP/gRPC 쿼리, 재시작 후 데이터 유지, 백업·검증·복원과 복원 데이터 읽기 |
| 복구 fixture 소요 시간 | `data.redb` **294,912 bytes**; 생성 **0.084초**, 검증 **0.028초**, 복원 **0.055초** |
| 시작 fixture 소요 시간 | 최초 **0.267초**, 재시작 **0.237초**, 복원 데이터 **0.244초** |
| 문서 | 영문→한국어 mdBook 순차 빌드와 14쌍 code/status/link 검사 PASS |
| 호스팅 게시 | **PASS**: CI·auto-release 성공, Linux/macOS x86_64/arm64 archive 4개에 각각 server/CLI/backup과 LICENSE/NOTICES 포함. macOS binary signing/notarization 및 모든 target의 압축 해제 runtime smoke 통과; Linux native, macOS x86_64 Rosetta 실행 |
| 게시 artifact 검증 | **PASS**: archive 4개를 직접 내려받아 release asset SHA-256, 정확히 5개 member와 binary 12개의 architecture 확인; macOS 서명 6개 검증. 다운로드한 macOS arm64 binary는 v0.4.23/`fb96cb3`을 보고하며 HTTP/gRPC 재시작·백업/복구 smoke 통과 |

위 로컬 build·복구 소요 시간은 인수 fixture의 측정값이며 운영 처리량이나 보편적인
RPO/RTO가 아닙니다. 호스팅 workflow 링크는 이후의 문서 전용 commit이 아닌 실제 검증·
게시된 릴리스를 가리킵니다.

## 구현된 인수 경계

| 영역 | 계약 | 실행 검증 |
|---|---|---|
| 쿼리 완전성 | LOOKUP/FIND/MATCH의 budget 초과와 저장소·decode 오류를 명시적으로 반환; SQL LIMIT으로 불완전한 후보 scan을 숨기지 않음 | Executor completeness suite와 실제 Meta + Storage RPC 두 개의 fixture |
| 분산 component | 모든 partition 식별자·host 검증, query에 묶인 keyset cursor로 index 순회, 원격 payload 조회, partition 실패·손상 전파 | `distributed_completeness_tests.rs`; 지원되는 복제 launcher를 의미하지 않음 |
| DML | 한 문장의 current data, asserted history, index, vector, counter, sameAs 변경을 준비한 뒤 Memory/redb에서 한 번에 commit | `dml_atomic_tests.rs`의 byte-exact 실패·취소 보존과 동시 DDL |
| 추론 | Assertion과 영속 미완료 marker를 함께 commit; 읽기·쓰기를 조정하고 closure 재구축 후 해제 | `inference_recovery_tests.rs`의 실패 주입, 취소, 동시 repair, redb 재시작 |
| 복구 | 격리 사본에서 manifest·digest·크기·redb 내용 검사; 교체 전 복원 사본 검증 | Backup 실패 주입과 `tests/backup_recovery_test.rs`의 실제 CLI 복구 |
| Lifecycle | 두 API bind 후 저장소·readiness 시작, 비정상 종료 감시, auth 대기 전 요청 계수, checkpoint/drain 실패 전파 | Graph admission race, `server_cli_test.rs`, checkpoint 실패 주입, 실제 binary smoke |
| 전송 | TLS HTTP/gRPC proxy, 비공개 backend port, 공개 운영 route 차단, 비root container, 인증서 교체 | `deploy/secure/smoke.py --build` |
| 패키지 | 정확한 ref의 audit/fmt/check/Clippy/test, server/CLI/backup 필수, license/notices를 포함한 archive 4개 | CI/release workflow와 압축 해제 artifact의 `scripts/release_smoke.py` |

Archive smoke는 두 protocol의 인증, index가 있는 fact 쓰기·읽기, read-only 변경과
미지원 HTTP parameter 거부, SIGTERM, 재시작, 백업 생성·검증·복원과 동일 fact 읽기를
실행합니다. Server log에 credential과 query value가 없는지도 검사합니다.
Linux target은 native로, macOS x86_64는 ARM runner의 Rosetta로 실행합니다.

## 추론 운영과 결과 불확실성

```ngql
USE my_space;
CHECK INFERENCE;
REPAIR INFERENCE;
```

`CHECK INFERENCE`는 일반 읽기 권한으로 `complete` 또는 `repair_required`를 반환합니다.
`REPAIR INFERENCE`는 GOD/ADMIN을 요구하며 compound·PROFILE을 포함한 read-only
요청에서는 거부합니다. Dirty space는 repair가 성공할 때까지 graph 읽기와 추가 변경을
거부합니다. Repair는 반복 실행할 수 있으며 asserted history를 보존하고 오래된
derived/reverse/type/provenance를 제거한 뒤 현재 closure를 재구축합니다. O(graph)
작업이며 semantic mutation 뒤에도 같은 전체 재구축 전략으로 완결합니다.

Assertion commit과 closure 완성은 별도 transaction입니다. 추론 오류·취소·응답 유실이
assertion rollback을 의미하지 않습니다. Repair 뒤 상태와 현재 fact를 확인한 후 재시도를
판단하세요. 영속 idempotency key나 mutation-outcome API는 없습니다. Compound는
절별 commit이며 단일 문장 DML 원자성은 compound transaction이나 추론 전체 효과의
transaction이 아닙니다. Embedded custom KV backend도 Memory/redb와 같은 보장을
제공하려면 원자적인 `batch_apply`를 구현해야 합니다.

Inferred-only edge의 UPDATE는 명시적 assertion으로 승격합니다. 직접 DELETE는 거부하므로
근거 assertion을 삭제하세요. 사용자는 예약된 `__inferred__` marker를 설정할 수 없습니다.
SameAs 병합은 비가역적입니다. Subclass, equivalent/disjoint, DOMAIN/RANGE, shape,
tag index가 참조하는 class는 삭제할 수 없습니다. ALTER CLASS가 없으므로 상호 동등한
class는 참조가 유지되는 동안 개별 삭제가 불가능합니다.

## 배포·복구 제한

- [TLS 배포 절차](../book/src.ko/operations/secure-deployment.md)를 사용하세요. Global role은
  상호 비신뢰 tenant를 격리하지 않으므로 별도 store가 필요합니다. Launcher는 기본값이 아닌
  cluster 설정을 거부합니다. Native TLS·HA는 미지원이며 기존 AKS manifest에는 암호화한 ingress가 없습니다.
- Float equality는 기존 epsilon 비교를 보존하기 위해 선택한 index 전체를 재검증하며 모든
  후보가 scan budget에 포함됩니다. Int/Float equality는 타입을 구분합니다. Index cursor는
  동시 쓰기 사이 snapshot을 제공하지 않으며 분산 full scan/range의 기존 제한은 유지됩니다.
- Offline backup 검증은 손상을 검출하며 외부 사실의 진실성이나 archive 인증을 제공하지
  않습니다. [백업·복구](../book/src.ko/operations/backup.md)를 참고하세요. 두 directory 교체는
  일반 오류 rollback을 지원하지만 crash-atomic하지 않습니다. 중단된 overwrite 뒤
  `.byoridb-restore-*/previous`를 보존하세요.
- HTTP query body의 알 수 없는 field는 422로 거부합니다. Typed parameter, 모든 값의
  정확한 JSON INT64 표현, 일반 result/evidence envelope는 후속 API 작업입니다. Session ID는
  이미 decimal string으로 전달합니다.
- Workload에 맞춰 result/staging/scan limit을 설정하세요. 정확성 fixture가 처리량이나
  복구 시간 보장은 아니므로 대표 데이터로 측정해야 합니다.

## 검증 명령

```bash
cargo fmt --all --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -- --test-threads=1
cargo build --locked --release --workspace
cargo audit --file Cargo.lock
BYORIDB_RELEASE_VERSION="$(python3 -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])')"
python3 scripts/release_smoke.py --bin-dir target/release --version "$BYORIDB_RELEASE_VERSION"
python3 deploy/secure/smoke.py --build
mdbook build book
mdbook build book/ko
```

버전 조회 명령은 Python 3.11 이상이 필요합니다. 게시 build는 archive smoke에 `--commit`도
넘겨 clean build SHA 일치를 요구합니다. 해당 SHA의
[CI](https://github.com/byoridb/byoridb/actions/workflows/ci.yml),
[auto-release](https://github.com/byoridb/byoridb/actions/workflows/autorelease.yml),
[게시 artifact](https://github.com/byoridb/byoridb/releases)를 확인하세요.
