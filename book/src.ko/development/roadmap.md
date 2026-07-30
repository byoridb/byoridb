# 로드맵

[English](../../development/roadmap.html) | **한국어**

ByoriDB의 현재 상태를 요약합니다. 우선순위와 완료 근거의 단일 진실원은
[docs/PLAN.md](https://github.com/byoridb/byoridb/blob/main/docs/PLAN.md)입니다.

## 구현된 핵심

- nGQL DDL/DML과 `MATCH`, `GO`, `FETCH`, `LOOKUP`, `FIND PATH`
- redb 기반 current view와 원자적 bitemporal history 기록
- asserted vertex/edge `FETCH ... AS OF <epoch-ms>`
- class hierarchy, RDFS-Plus/선택적 OWL 2 RL materialization, `WHY`, DRed, shape 검사
- 구조·embedding(HNSW)·hybrid recommendation
- HTTP/gRPC/CLI, Prometheus metrics, full snapshot backup/restore
- Raft, partition allocator, failure detector 등 분산 라이브러리 구성요소

## 우선 남은 기능

- temporal `MATCH`/`GO`, `VALID FROM/TO`, `BETWEEN`, history API, retention/GC와
  과거 추론 사실
- Storage/Raft bootstrap을 포함한 실제 multi-node launcher와 배포 wiring
- `FIXED_STRING` VID 실행 경로, edge `LOOKUP`, FIND `WHERE/YIELD`, MATCH edge accessor
- 실제 balance-job 제어 RPC (`BALANCE`는 현재 명시적 unsupported 오류)
- TLS, Grafana/알람 규칙과 중앙 로그 수집

이미 완료된 기능을 오래된 버전 번호로 다시 분류하지 않습니다. 릴리스 상태는 Git 태그와
README의 release 주의를 함께 확인하세요.

## 기여하기

기능 개발에 기여하고 싶으신가요? [기여 가이드](./contributing.md)를 확인하세요.

## 기능 요청

아이디어가 있으신가요? `enhancement` 라벨을 붙여 이슈를 열어 주세요.
