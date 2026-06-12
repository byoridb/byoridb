# ByoriDB Plan

마지막 업데이트: 2026-05-29 (프로젝트 방향 = 온톨로지 DB 확정. O 섹션 신설)

이전의 `ROADMAP.md` / `docs/NEXT_STEPS.md` / `docs/MOCK_REMEDIATION_PLAN.md` /
`docs/GRAPH_ALGORITHM_OPTIMIZATION_PLAN.md` 4개 문서를 통합한 **단일 진실원**.
세션 시작 시 이 문서 하나만 보면 된다.

---

## 프로젝트 방향 (2026-05-29 확정)

**ByoriDB의 목표는 property graph DB가 아니라 온톨로지 DB다.**

- **코어 모델**: 기존 property graph 코어(KV+Raft+그래프 엔진)를 **유지**하고,
  그 위에 **시맨틱 레이어**(클래스 계층 + 추론 + 시맨틱 관계)를 얹는다.
  RDF triple store로 전면 재설계하지 않는다 — 기존 자산을 재활용하는 점진적 경로.
  (참조 제품: Stardog, Neo4j+n10s, GraphDB)
- **추론(inference) 전략**: **미정 — 리서치 필요.** 쿼리타임 추론 vs
  materialization(사전 추론) vs 하이브리드 중 결정해야 함. 이것이 온톨로지 DB
  성능의 핵심 갈림길. O-0 참조.
- **경쟁 상대 재정의**: NebulaGraph(property graph)는 **올바른 벤치 대상이 아니다.**
  진짜 비교군은 추론 엔진을 가진 triple store(Stardog, GraphDB/Ontotext,
  Apache Jena, Virtuoso). 단, property graph 성능 자체(MATCH/GO)는 온톨로지
  추론의 하부 연산이므로 NebulaGraph 대비 개선은 계속 유효하다.

이 방향에서 **온톨로지 핵심 기능은 현재 0% 구현**이다(추론/클래스계층/시맨틱관계
코드 전무). 남은 작업 **O 섹션**이 최우선 신규 트랙이다.

---

## 현재 상태

ByoriDB는 단일 노드와 분산 클러스터 모두에서 동작한다. 운영 배포 로드맵
Phase 1–10이 모두 완료되었고, mock/hardcoded 청산(PR 1–10) 및 그래프 알고리즘
최적화(Phase 0–7)도 종결되었다.

**검증된 기능**

- 단일 노드: WAL, graceful shutdown, 인증(root/role-based), WHERE 절,
  edge CRUD, Prometheus 메트릭, 구조화 JSON 로그
- 분산: Raft 합의(커스텀 구현, openraft 호환성 이슈 우회), Consistent Hash Ring,
  Replica Factor, 자동 failover, FailureDetector, 파티션 재분배, 핫스팟 감지
- 쿼리: nGQL DDL/DML/DQL, MATCH 패턴(다중 hop 노드 필터), FIND PATH
  (unweighted BFS + weighted Dijkstra), GO multi-step, LOOKUP, 인덱스
  (tag/edge, multi-field, prefix-cover 매처), 분산 LOOKUP, 분산 GO
  (source-VID targeted RPC, O(degree)), compound statement
  (`$var = stmt; stmt2`)
- 운영: 백업/복구(`byoridb-backup`), 부하 테스트(`load_test` — 31K QPS @ 50동시,
  12.5K QPS @ 100동시, 0% 에러), 장애 복구(WAL 재생 15초 이내)

**워크스페이스 테스트**: 519개 통과(2026-05-13 기준).

**벤치 기반선** (`byoridb-executor/benches/graph_traversal.rs`, criterion full sample):

| 시나리오 | 시간 | Phase 3 이후 |
|---|---:|---:|
| BFS chain_far/4096 | 1.70 ms | -39% |
| BFS chain_far/16 | 5.30 µs | -49% |
| star_hub 16384 neighbors | 2.12 ms | -27% |
| Dijkstra weighted/4096 | 2.61 ms | -6.7% |

---

## 알려진 제약

- **Geography 디코딩**: 인코딩만 구현. WKB/WKT 파싱은 미구현(`byoridb-codec/src/row.rs`). 외부 요구 발생 시 진행.
- **모니터링 대시보드**: Prometheus 메트릭은 `/metrics`로 노출되지만 Grafana 템플릿/알람 규칙이 없음.
- **로그 수집 파이프라인**: JSON 로그는 출력되지만 Filebeat/Fluentd 같은 중앙 집중 파이프라인 연동 설정이 없음.
- **트랜잭션**: 분산 그래프 DB의 2PC 비용이 크다고 판단해 의도적으로 미지원.
- **MATCH pattern reorder**: 가장 selective한 노드부터 시작하도록 자동 reorder가 없음.
- **label-only MATCH tag-vid 인덱스** ✅ 적용 완료 (2026-05-29): INSERT VERTEX 시 `{space}:tagvid:{tag_name}:{vid}` 보조 인덱스 작성. label-only MATCH 패턴 (`MATCH (p:product)`) 에서 전체 vertex 스캔 대신 tag-vid prefix scan 사용. 기존 데이터(인덱스 기록 전 삽입)는 빈 scan → 풀스캔 폴백 보장. NebulaGraph 벤치마크 대비 MATCH 157배 지연 원인이었음.
- **`byoridb-server` bin standalone 한계**: `src/main.rs`/`src/config.rs`가 Storage+Graph+HTTP를 같은 프로세스로 standalone 실행만 노출. 분산 클러스터(Raft/peer/cluster ID) 설정을 binary 레벨에서 받지 않음. 라이브러리에는 Raft 합의 구현이 있고 PLAN.md 검증된 기능에도 분산이 포함되어 있으나, `byoridb-server`를 그대로 multi-replica로 배포하면 실제로는 독립된 N개 단일 노드가 됨(docker-compose.yml의 3 컨테이너도 마찬가지). 분산 배포가 필요하면 G-2 작업 선결 필요.
- **빌드 환경 MSRV**: `base64ct 1.8.3`(transitive)이 Rust edition2024를 요구해 Rust ≥ 1.85 필요. Dockerfile은 2026-05-13에 `1.80 → 1.86`으로 갱신됨. MSRV가 어디에도 명시되어 있지 않아 CI/Docker 환경 드리프트 시 다시 깨질 수 있음.

---

## 남은 작업 (TODO)

### O. 온톨로지 DB 레이어 (P0 — 프로젝트 핵심 방향, 2026-05-29 신설)

property graph 코어 위에 시맨틱 레이어를 얹어 온톨로지 DB로 만든다.
**현재 구현 0%.** 의존성 순서대로 정렬(아래로 갈수록 위 항목에 의존).

**O-0 [선결, 리서치] 추론 전략 결정** ✅ 리서치 완료 (2026-05-29, deep-research)

**결정된 전략 (문헌 합의 기반):**
- **Materialization (forward chaining) 우선.** 쿼리타임 backward chaining 아님.
  GraphDB·RDFox·Oracle·OwlOntDB 모두 DB-resident 추론에 materialization을
  주 전략으로 채택. 쓰기 시 추론 결과를 미리 계산·저장 → 읽기는 빠름.
  (반대 진영: Stardog·Virtuoso·AllegroGraph = backward chaining/query-rewriting)
- **타겟 프로파일: OWL 2 RL.** W3C가 datalog로 매핑 가능하도록(PTIME, 규칙 기반)
  의도적으로 설계한 프로파일. RDFox·GraphDB·Oracle·OwlOntDB의 사실상 표준.
- **시작 규칙 셋: RDFS-Plus** (full OWL 2 RL 전에). subClassOf/subPropertyOf
  transitivity + `owl:TransitiveProperty` + `owl:inverseOf` +
  `owl:SymmetricProperty` + domain/range. GraphDB의 기본 ruleset이자 가치/비용
  최적점. **`owl:sameAs`(동치)는 가장 비싼 outlier라 마지막으로 미룸.**
- **증분 갱신: 삭제를 미룬다.** 추가(insertion)는 seminaive로 단조 처리 쉬움.
  삭제는 "더 이상 도출 안 되는 사실을 모두 찾아 retract"해야 해 근본적으로 비쌈.
  1단계: full re-materialization 또는 insertion-only 증분.
  2단계: B/F(Backward/Forward) 알고리즘 도입(DRed 아님 — B/F는 bookkeeping
  불필요 + 정확 삭제, DRed의 overdeletion 회피).
- **분산 materialization은 마지막·최난도 마일스톤.** 강력한 실증 결과는 전부
  단일 노드 shared-memory(RDFox 16코어 13.9배). 분산 datalog materialization은
  미검증 영역. ByoriDB 분산 모드(G-2)도 미완성이라 더욱 후순위.

**구현 난이도 순서**: 단일노드 RDFS-Plus materialization → full re-mat/
insertion-only 증분 → B/F 삭제 증분 → (먼 미래) 분산 materialization.

**미해결/추가조사 필요** (리서치 caveat):
- **LPG 위 추론 선례가 가장 약하게 커버됨** — OwlOntDB는 RDB 타겟, 나머지는 RDF
  triple store 타겟. property graph(LPG) 엣지 위에서 OWL 2 RL materialization을
  직접 한 검증 사례가 부족. Neo4j n10s/Neptune/TigerGraph의 LPG 추론 방식·한계
  재조사 필요(part 4 미답).
- Stardog/Virtuoso/Neptune/Jena의 backward-chaining 구현은 2차 자료로만 확인됨.
- `owl:sameAs` 동치 추론의 LPG 상 비용·처리법.

**O-1 [P0, 선결 부채] 역방향 edge 인덱스** ✅ 완료 (2026-06-09)
LDBC Q8 `GO FROM <many ids> OVER reply_of REVERSELY`가 120s timeout으로 발동.
`algo::get_incoming_neighbors`가 `{space}:edge:` 전체를 스캔해 dst로 필터하던
O(전체 엣지) 풀스캔(src 25개 fan-out × 1.48M edge ≈ 3,700만 디코드)이 원인.
수정: `{space}:in-edge:{dst}:{edge_type}:{src}:{ranking}` 역방향 인덱스 도입(value는
정방향과 동일한 denormalized edge payload). `get_incoming_neighbors`는 이제
`{space}:in-edge:{dst}:` prefix scan → **O(in-degree)**, 정방향과 대칭. INSERT
EDGE가 같은 batch로 양방향 기록, DELETE EDGE가 양쪽 삭제(UPDATE는 vertex 전용이라
무관). MATCH reverse single-edge 최적화도 자동 수혜 + EXPLAIN을 reverse-edge index로
정확히 표시(더 이상 FULL SCAN 오표시 안 함). `SchemaKey::in_edge_data` + 유닛/통합
테스트 추가. **기존 데이터는 역방향 entry가 없으므로 space 재로드 필요**(SF0.1
재로드로 처리, 백필 마이그레이션은 미구현).

**O-2 [P0, 선결 부채] 변길이 경로 `*1..n` 실행** ✅ 완료 (2026-06-12, 트랙 1+2
구현·배포·프로덕션 실증 — 운영 완료로 종결)
파서·AST(`EdgePattern.range`)는 있으나 실행기 미구현. transitive closure가
본질적으로 변길이 경로이므로 온톨로지 추론의 하부 연산.

LDBC Q13/Q14(shortest path 계열)가 하네스 포팅 한계로 엔진 과제로 분리되며
설계 확정(2026-06-12). **두 트랙으로 분할, 트랙 1 선행:**

*트랙 1 — FIND PATH 확장 (LDBC Q13/Q14 unblock)* ✅ 구현·배포 완료
(2026-06-12, 커밋 bca1beb+de0880f, main 머지 → AKS 배포 sha-de0880f.
프로덕션 SF0.1에서 Q13 알려진 쌍 length=3, Q14 알려진 쌍 단일 경로 일치
스모크 통과)
- 파서: `FIND ALL SHORTEST PATHS`(`PATH` 단수형도 허용) + `BIDIRECT` +
  `UPTO n STEPS`(양수 검증, `max_go_steps` 상한 — 초과 시 에러). lexer에
  `UPTO`/`PATHS` 토큰 추가.
- `algo_paths.rs` 신설: `shortest_path`(단일, bidirect 지원) +
  `all_shortest_paths`(레벨 동기 multi-parent DAG → 전 경로 열거).
  bidirect 확장은 forward `{space}:edge:` + O-1 `{space}:in-edge:` 양쪽
  prefix scan. codec에 `decode_edge_src` fast path 추가(in-edge 값에서
  src만 varint 추출). `max_traversal_nodes` 캡이 mid-level에 걸리면
  부분 결과 대신 빈 결과(+cap_reached) — 부분 all-paths는 오답이므로.
  경로 수 상한 `ExecutionConfig.max_find_paths`(기본 1,024, 초과 시 warn).
- 결과 포맷: `path` 컬럼 `Value::List(vids)`(기존 `"1->2->3"` 문자열 폐기).
  HTTP에선 JSON 배열로 직렬화. Q13=하네스가 len-1, Q14 weight 스코어링은
  하네스 측(message 레벨 조인이라 엔진 범위 아님, 쌍 단위 캐시).
- **부수 수정: `Value::PartialEq`가 List/Map/Set/Path/Date/Time/DateTime/
  Duration에서 무조건 `false`** (자기 자신과도 불일치) → 구조 비교 추가.
- 제약: `WEIGHT BY`는 `BIDIRECT`/`ALL SHORTEST PATHS`와 조합 불가(에러).
  EXPLAIN이 "BFS all shortest paths BIDIRECT" + reverse-edge index 표시.
- 미해결: O-1과 동일한 in-edge 백필 caveat — 역방향 entry 없는 기존
  데이터에선 BIDIRECT가 빈 결과. FIND의 WHERE/YIELD 절은 여전히 미구현
  (파싱만 됨, 기존과 동일).

*트랙 2 — MATCH `*min..max` 본체 (`match_impl/var_length.rs` 신설)* ✅ 구현
완료 (2026-06-12, 커밋 6336ecd 배포. 프로덕션 실증: SF0.1 무방향 knows
`*1..2` 401 vertex가 GO forward+REVERSELY 독립 계산과 정확 일치, EXPLAIN
var-length 표시 확인)
- `match_edges`가 range 존재 시 `expand_var_length`로 위임. DFS 명시 스택,
  경로 내 visited-vertex로 사이클 차단(transitive closure 의미론). 결과는
  **distinct terminal vertex 단위**(경로당 1행이 아님 — Cypher와의 차이,
  중간 노드 미바인딩).
- 방향 3종 지원 + **기존 버그 수정**: `EdgeDirection::Undirected`가
  `match_edges`에서 Outgoing으로 처리되던 것을 forward+in-edge 합집합으로
  (`neighbors_for_direction` 헬퍼, 고정 hop에도 적용).
- max > `max_go_steps`(20) 시 에러, `max_traversal_nodes` 캡 도달 시 부분
  결과+warn. 변길이 edge variable 바인딩(`[e:t*1..2]`)은 명시 에러로 거부
  (1단계 미지원). EXPLAIN Expand에 `var-length *min..max` 표시.
- 회귀 8건: chain/min>1/cycle 종료/terminal filter/캡 에러/edge var 거부/
  무방향 단일 hop(역방향 저장)/무방향 변길이 혼합 방향.
- **미해결(후속)**: Undirected 사용 데이터도 in-edge entry 필요(O-1 백필
  caveat 동일). per-path 행/edge list 바인딩은 Cypher 호환 필요 시 후속.

**O-3 [P1] 클래스 계층 / TBox 모델링** 🔶 설계 완료 (2026-06-12), 구현 미착수
스키마를 "태그"가 아니라 "클래스 + subClassOf 관계"로 표현.

*설계 결정 (2026-06-12):*
- **D1. 클래스 = tag의 상위 호환(superset), 메타/스키마 평면.**
  `CREATE CLASS dog(props...) [SUBCLASS OF animal[, pet]]`는 (1) 일반 tag
  정의(`space:{space}:tag:dog`)를 그대로 기록하고 (2) 클래스 메타
  `space:{space}:class:dog` → `{name, superclasses: [...]}`를 추가.
  INSERT VERTEX / MATCH label / tag-vid 인덱스 / S-4 검증 전부 무변경
  재사용. 기존 tag는 "계층 비참여 클래스"로 공존. (대안 기각: 별도
  네임스페이스는 INSERT/MATCH/인덱스 중복 구현 필요.)
- **D2. LDBC tag_class/is_subclass_of와 분리.** 그것은 사용자 데이터
  평면(ABox 정점/엣지)이고 O-3는 스키마 평면(TBox). 데이터 레벨 계층
  순회는 O-2 변길이 경로로 이미 가능(`-[:is_subclass_of*1..n]-`).
  데이터 레벨 시맨틱(transitive 등)은 O-4의 edge-type 메타데이터 담당.
- **D3. 저장/조회.** 다중 상속 허용(RDFS subClassOf 다중 가능). ancestors
  = superclass 체인 walk(깊이 캡 16), descendants = `class:` prefix scan
  후 메모리 역인덱스(클래스 수는 적음). 캐시는 index-def 영속화(OnceCell
  lazy load) 패턴 재사용 — **graph 서비스가 쿼리마다 ctx를 새로 만들던
  함정(2026-06-10 버그) 재발 주의.** standalone 우선: executor가
  CREATE TAG와 동일하게 KV 직접 기록(분산 meta 연동은 G-2 이후).
- **D4. DDL 표면.** `CREATE CLASS name(props) [SUBCLASS OF p1[, p2]]`
  (+IF NOT EXISTS), `DROP CLASS [IF EXISTS]`(자식 존재 시 거부 RESTRICT),
  `SHOW CLASSES`(name/superclasses), `DESCRIBE CLASS`(props+superclasses+
  ancestors). ALTER CLASS(SUBCLASS 변경)는 1단계 보류.
- **D5. 무결성.** SUBCLASS OF 부모는 *클래스*로 존재해야 함(tag 지정 시
  에러 — tag는 계층 비참여). CREATE 시 ancestor walk로 사이클 거부
  (자기참조 포함), 깊이 캡 초과 에러. DROP SPACE cleanup에 class 키 포함
  (PR#9 DROP SPACE 선례).
- **D6. 최소 회귀.** create/drop/duplicate/IF NOT EXISTS, subclass chain
  ancestors, 다중 부모, 사이클 거부(직접+간접), 자식 있는 DROP 거부,
  SHOW/DESCRIBE, CREATE CLASS 후 INSERT VERTEX·MATCH가 tag처럼 동작.
- **O-7 연계(범위 외).** `MATCH (x:animal)`의 추론 포함 매칭(descendants
  확장)은 O-5/O-7에서. O-3는 저장+DDL+introspection까지.

**O-4 [P1] 시맨틱 관계 타입** ⬜ 미착수
`subClassOf` / `subPropertyOf` / `transitiveProperty` / `inverseOf` /
`sameAs` 등 메타 관계를 1급 시민으로. O-3 클래스 계층과 함께 설계.

**O-5 [P1] 추론 엔진 (O-0 결정 반영)** ⬜ 미착수
O-0 결정에 따라 **RDFS-Plus 규칙의 forward-chaining materialization**으로 시작.
edge를 datalog-style 규칙으로 추론 → 결과를 KV에 미리 저장. transitive
closure(O-2 변길이 경로 활용), subclass/subproperty, inverse/symmetric,
domain/range 순. **증분 갱신은 insertion-only부터, 삭제는 후속(B/F 알고리즘).**
`sameAs`는 마지막. 분산 materialization은 별도 먼 마일스톤. O-1·O-2가 선결.

**O-6 [P2] 일관성 검사 (consistency / validation)** ⬜ 미착수
온톨로지 모순 탐지(disjoint class 위반, domain/range 위반 등). SHACL/OWL
validation 참고.

**O-7 [P2] 시맨틱 쿼리 표면** ⬜ 미착수
nGQL 확장으로 추론 쿼리 노출(예: `MATCH ... WHERE c IS-A Animal`처럼
추론 포함 매칭). SPARQL 호환은 별도 검토(하이브리드 모델 아님 — 보류).

### S. 보안 강화 (P0, 즉시 — 2026-05-13 심층 분석 결과)

2026-05-13 코드 심층 분석에서 발견된 이슈. Critical/High 우선 순서로 진행.

**S-1 [Critical, M] RBAC 권한 검증 적용** ✅ 완료 (2026-05-13)
`check_permission()`이 구현되어 있으나 한 번도 호출되지 않음. GUEST가 DROP SPACE, GRANT GOD 등 모든 명령 실행 가능.
- `byoridb-graph/src/service.rs` — Statement→Permission 매핑 + check_permission 호출
- `byoridb-executor/src/executor.rs` — CREATE/DROP USER, GRANT, REVOKE에 caller role 검사

**S-2 [Critical, S] 세션 ID 랜덤화** ✅ 완료 (2026-05-13)
`AtomicI64` 순차 증가 → 세션 탈취 가능. S-1과 함께 수정해야 RBAC 우회 차단.
- `byoridb-graph/src/session.rs` — `AtomicI64` 제거, OsRng 기반 random i64

**S-3 [Critical+High, M] AUTH-SYNC write-through** ✅ 완료 (2026-05-13)
CREATE USER/GRANT/REVOKE가 AuthManager in-memory 캐시에 미반영. 기존 "알려진 제약"에서 승격.
- `byoridb-graph/src/service.rs` — 실행 성공 후 sync_auth_manager() 호출

**S-4 [Critical, L] INSERT/UPDATE 스키마 검증** ✅ 완료 (2026-05-13)
존재하지 않는 tag/field에 어떤 타입이든 삽입 가능. 데이터 무결성 보장 불가.
- `byoridb-executor/src/executor.rs` — validate_tag_props/validate_edge_props 추가

**S-5 [High, M] scan_prefix 무제한 스캔 + GO 팬아웃 상한** ✅ 완료 (2026-05-13)
단일 쿼리로 OOM/서버 다운 가능.
- `byoridb-executor/src/context.rs` — max_scan_limit=100_000, max_go_steps=20
- `byoridb-executor/src/match.rs` — scan_prefix_limited 사용
- `byoridb-executor/src/executor.rs` — GO step 상한 검사

**S-6 [High, S] set_null_flag panic 수정** ✅ 완료 (2026-05-13)
nullable 필드 0개 스키마에서 서버 crash.
- `byoridb-codec/src/row.rs` — bounds check guard 추가

**S-7 [High, S] delete_session 소유권 검증** ✅ 완료 (2026-05-13)
타인 세션 강제 종료 가능(DoS).
- `byoridb-graph/src/service.rs` — sign_out(caller, target) 소유권 검증

**S-8 [High, S] WAL checksum → CRC32C** ✅ 완료 (2026-05-13) (기존 B항목과 통합)
`wrapping_add` 단순 합은 바이트 swap 미탐지. `crc32fast` 도입.
- `byoridb-kvstore/src/wal.rs`

**S-9 [Medium, S] 메시지 크기 제한 + zip bomb 방어** ✅ 완료 (2026-05-13)
- `byoridb-graph/src/server.rs` — max_decoding_message_size=64MB

**S-10 [Medium, S] 백업 파일 권한 0o700** ✅ 완료 (2026-05-13)
- `byoridb-kvstore/src/backup.rs`

**S-11 [Medium, S] Meta HTTP 바인딩 제한 + /metrics 접근 제어** ✅ 완료 (2026-05-13)
- `byoridb-meta/src/server.rs` — 127.0.0.1 바인딩으로 변경

**S-12 [Medium] TLS — 의도적 미구현, 네트워크 격리로 대체**
유동 IP 환경에서는 IP SAN 인증서 관리 비용이 크고, 대부분의 DB 배포(Redis, PostgreSQL 등)가 네트워크 격리에 의존하는 것과 동일한 패턴.
- **운영 필수 조건**: VPC/내부망 격리 + 방화벽/보안 그룹으로 외부 접근 차단
- 퍼블릭 네트워크 노출이 필요한 경우에는 앞단에 TLS 종료 프록시(nginx, envoy) 배치 권장
- 규정 준수(PCI-DSS, HIPAA) 요구 시 재검토

**S-13 [Medium, M] RocksDB 메모리 상한 설정** ✅ 완료 (2026-05-13)
`max_memory_mb` 선언만 되고 미사용. 대량 쓰기 시 OOM.
- `byoridb-kvstore/src/store.rs` — write_buffer_size=64MB, max_write_buffer_number=3

**S-14 [Low, S] 해싱 중복 제거 + 기타 소형 수정** ✅ 완료 (2026-05-13)
- `byoridb-graph/src/auth.rs` — hash/verify를 byoridb_common::crypto로 위임
- `byoridb-meta/src/rpc.rs` — DataType silent fallback → warn 로그
- `byoridb-codec/src/row.rs` — Geography bounds check

**S-15 [High, M] Brute-force 방어** ✅ 완료 (2026-05-13)
로그인 실패 횟수 제한 없음. IP/username 기반 rate limiting.
- `byoridb-graph/src/auth.rs` — 5회 실패 시 5분 잠금, 성공 시 카운터 초기화

**S-16 [High, S] Heartbeat 스푸핑 방어** ✅ 완료 (2026-05-13)
Meta gRPC heartbeat에 인증 없음. 가짜 노드 클러스터 등록 가능.
- `byoridb-meta/proto/meta.proto` — HeartbeatRequest에 cluster_id 필드 추가
- `byoridb-meta/src/service.rs` — cluster_id 검증 (0은 최초 등록 허용)

**S-17 [Medium, S] 세션 sliding window** ✅ 완료 (2026-05-13)
`last_accessed` 필드 있으나 `expires_at` 미갱신. 장시간 작업 중 세션 끊김.
- `byoridb-graph/src/session.rs` — get_session 호출 시 expires_at 연장

**S-18 [Medium, S] HTTP 쿼리 문자열 길이 제한** ✅ 완료 (2026-05-13)
HTTP API에 쿼리 크기 제한 없음 (gRPC는 64MB 제한 있음).
- `byoridb-graph/src/server.rs` — MAX_QUERY_LEN=1MiB, 초과 시 413 반환

### A. 운영 도구 연동 (P1, 운영 시작 전 필수)

- Grafana 대시보드 템플릿 + 알람 규칙 (Prometheus 연동)
- 로그 수집 파이프라인 (Filebeat/Fluentd → ELK/Loki 검토)

### B. KVStore 성능 후속 (P2, 측정 기반)

> **2026-06-05 redb 전환으로 이 섹션 대부분 무효화.** RocksDB + 외부 WAL(wal.rs,
> WalKVStore)을 제거하고 순수 Rust **redb**로 단일화 (C++ 툴체인 의존 제거,
> clean build ~28s). redb 자체 ACID(`Durability::Immediate` = commit마다 fsync)가
> durability를 제공하므로 이중 WAL 최적화/CRC32C 항목은 자연 해소됨.
> `benches/wal_overhead.rs`도 삭제.

- **redb 쓰기 처리량 재측정** — redb는 단일 writer 직렬화 + commit fsync(Immediate)라, 고빈도 단일 `put`이 (구) RocksDB memtable 버퍼링보다 느릴 수 있음. 핫패스는 `batch_put`로 묶고, 필요 시 `Durability::Eventual` 옵션 노출 검토(`KVStoreOptions.use_fsync` 자리에 예약됨).
- ~~RocksDB 내부 WAL 비활성~~ — 무효 (RocksDB 제거).
- ~~WAL checksum CRC32C~~ — 무효 (외부 WAL 제거, redb 내부 체크섬 사용).

### C. 그래프 알고리즘 후속 (P2~P3, 워크로드 의존)

- **MATCH pattern execution reorder** — 가장 selective한 노드부터. semantic risk 큼.
- **LOOKUP range 술어 인덱스 미사용** ([#1](https://github.com/byoridb/byoridb/issues/1)) — `LOOKUP ... WHERE age > 30` 이 인덱스가 있어도 풀스캔으로 폴백. 실행기/플래너 모두 동등(Eq) 조건만 인덱스 경로로 라우팅(`execute_lookup`/`extract_eq_condition`, `explain::lookup_access`/`eq_field`). 근본: `IndexManager::lookup_tag` point-equality만 지원, range index scan 미구현. EXPLAIN/PROFILE 풀스캔 경고(dc5be3b)가 발견.
- **label-only MATCH reverse index** ✅ 완료 (2026-05-29) — `{space}:tagvid:{tag}:{vid}` 보조 인덱스 도입. INSERT VERTEX 에서 자동 기록, MATCH 에서 label-only 패턴 시 자동 사용.
- **역방향 edge 인덱스(incoming)** → **O-1로 승격(P0).** `get_incoming_neighbors` 풀스캔 문제. 온톨로지 transitive 추론의 선결 조건이라 C가 아닌 O 트랙에서 우선 처리.
- **변길이 경로 `*1..n` 실행** → **O-2로 승격(P0).** transitive closure 하부 연산.
- **distributed BFS multi-hop fanout** — 현재 `GO`는 단일 hop만 RPC. partition-local multi-hop RPC가 있으면 round-trip 감소.
- **edge dst-only stream decoding in storage RPC** — `GetNeighborsBySource`가 full `EdgeData` 반환. BFS-style 클라이언트가 dst만 필요할 때 wire payload 감축.
- **Dijkstra용 lightweight property fetch** — weight property만 필요한 경우 full decode를 피하는 codec 분기. Phase 3에서 BFS는 했지만 Dijkstra는 미적용(현재 −6~9%만 개선).

### D. 마이너 정확성/기능 (P3)

- **Geography WKB/WKT (Mock Item 12)** — 외부 요구 발생 시 진행.

*(완료: `RowWriter::set_null_flag` panic → S-6. AUTH-SYNC → S-3. ALTER DROP/CHANGE → 2026-05-13.)*

### E. 분산 시스템 후속 개선 (수요 대기)

- 쿼리 플래너: 파티션 프루닝(필요한 파티션만 조회)
- 분산 JOIN (Shuffle / Broadcast)
- 분산 집계 (COUNT/SUM 2단계)
- 분산 정렬 (ORDER BY + LIMIT 최적화)
- 쿼리 타임아웃 + 부분 결과 반환

### F. 응답 직렬화 확장 (PR 9 후속)

PR 9에서 `ExecuteResponse.result`가 구조화된 proto `DataSet`을 반환하지만,
복합 타입(Vertex, Edge, List, Map, Path, Set, DateTime, Geography 등)은
`json_value` 문자열로 폴백. 이를 first-class proto 메시지로 승격하는 작업.

### H. 조회 정확성 버그 (H-1~H-6 ✅ 해소)

**H-1, H-4, H-5 코드 수정 완료. H-2, H-3 재검증 결과 현재 코드에서 재현 불가.
H-6(콤마 멀티패턴 파싱 중단)은 2026-05-29 수정 완료 — 아래 참조.**

| 증상 | 상태 | 비고 |
|---|---|---|
| **H-1** Space ID 모두 0 | ✅ 수정 완료 | `allocate_space_id()` + space JSON에 id 저장 |
| **H-2** SHOW TAGS/EDGES 중복 | ✅ 재현 불가 | SchemaKey 기반 prefix는 space name으로 격리. AKS PV 잔존 데이터가 원인이었던 것으로 판단. 회귀 테스트 추가됨 |
| **H-3** GO 결과에 invalid dst=0 | ✅ 재현 불가 | edge key `{space}:edge:{src}:{type}:{ranking}` 콜론 구분자가 올바름. dst=0은 AKS PV 잔존 데이터 또는 이전 세션 corrupt edge 원인. 회귀 테스트 추가됨 |
| **H-4** LOOKUP vid=0 | ✅ 수정 완료 | proto 디코딩 fallback 추가 (`VertexCodec::decode_vertex`) |
| **H-5** FETCH PROP ON edge가 vertex 반환 | ✅ 수정 완료 | `FetchPlan`에 `is_edge_fetch` + `edge_refs` 추가, parser `src->dst` 파싱 |

**AKS 재배포 시 체크리스트**: 기존 PV 데이터가 있다면 클린 PV로 재시작 권장. 회귀 테스트(`test_h1_*`, `test_h2_*`, `test_h3_*`, `test_h4_*`, `test_h5_*`)가 CI에서 통과 중.

추가 발견 사항: edge identity는 `(src, edge_type, dst, ranking)` — 키 형식 `{space}:edge:{src}:{edge_type}:{dst}:{ranking}`. src가 같더라도 dst가 다르면 독립된 엣지로 저장된다. ranking은 동일 `(src, edge_type, dst)` 사이에 여러 엣지를 허용하는 용도. 배치 INSERT에서 dst가 키에 누락되어 마지막 엣지만 남던 버그는 2026-05-22에 수정됨.

**H-6 [Critical] MATCH 콤마 멀티패턴 파싱 중단 — WHERE/RETURN/LIMIT 소실** ✅ 수정 완료 (2026-05-29)

`MATCH (p)-[:e1]->(c), (p)-[:e2]->(t) WHERE ... RETURN ... LIMIT 10` 같은
콤마 연결 멀티패턴에서, `parse_match_pattern`이 첫 path만 파싱하고 콤마에서
조용히 멈췄다(파서에 `Pattern::Multiple` 생성 코드 부재). 그 결과 콤마 뒤
두 번째 패턴 + **WHERE·RETURN·LIMIT 절이 통째로 무시되어 `None`이 됨**.

**증상**: 벤치 Q4가 필터(`id(c)==X`)·LIMIT 없이 전체 product 반환 → 10만+
rows, 6.9초. "LIMIT pushdown 버그"가 아니라 **결과 정확성 버그**(필터 소실).

**수정 내용**:
- `byoridb-parser/src/parser/dql.rs` — `parse_match`가 콤마 루프로 추가 패턴을
  파싱해 `Pattern::Multiple`로 묶음. 이후 WHERE/RETURN/LIMIT 정상 도달.
- `byoridb-executor/src/match_impl/match_executor.rs` — `execute_match`가
  Multiple의 첫 패턴으로 main match 후, 나머지 패턴을 공유 변수(`p`) 기준
  **INNER join**(매칭 실패 시 base row drop, OPTIONAL의 left join과 대비).
  멀티패턴이면 row_limit 조기종료 비활성(조인 후 LIMIT 적용).
- 회귀 테스트: 파서 2건(`test_match_comma_multipattern_*`,
  `test_match_single_pattern_*`), 실행기 통합 2건(`h6_multipattern_tests`).

**미해결**: 독립(공유 변수 없는) 멀티패턴은 cross join으로 full scan — 정상
케이스는 공유 변수라 영향 적으나, 향후 패턴 reorder/조인 최적화 여지.

**2026-05-27 추가 수정 (BUG 5)**

| 항목 | 커밋 | 내용 |
|---|---|---|
| SESSION_EXPIRED 에러 코드 | TBD | HTTP: `GraphError::SessionNotFound` → `code:"SESSION_EXPIRED"` + HTTP 401. gRPC: `error_code: 2` (기존 1=일반 오류). proto 주석으로 error_code 의미 명시 |

**2026-05-26 추가 수정 (BUG 3 / BUG 4)**

| 항목 | 커밋 | 내용 |
|---|---|---|
| MATCH 빈 결과 | `1faec0f` | `matches_node`가 proto-encoded vertex를 `serde_json::from_slice`로 파싱해 항상 `false` 반환 → `VertexCodec::decode_vertex` 사용으로 수정 |
| CREATE INDEX `field(30)` 파싱 에러 | `1faec0f` | `parse_identifier_list`에 `field(length)` 선택적 힌트 처리 추가 |
| `CREATE TAG INDEX …` 문법 | `bd7c0f8` | `parse_create` dispatch에서 TAG+INDEX lookahead 추가. `CREATE INDEX TAG …` 기존 문법도 유지 |
| GO YIELD `$$.tag.prop` | `bd7c0f8` | `Expression::DstVertexProp` 파서+실행기 구현. GO 결과에서 destination vertex 프로퍼티 참조 가능 |
| GO YIELD `edge.prop` | `bd7c0f8` | `Expression::PropRef` 파서+실행기 구현. 마지막 hop 엣지 프로퍼티 참조 가능 |
| CRAP CI 병렬 race | `56d5df8` | config 테스트 `ENV_MUTEX`로 직렬화 |

### G. 배포/인프라 (P1, 운영 직전, 2026-05-13 Azure AKS 배포 시도 중 식별)

Azure AKS에 실제로 배포해보며 발견된 마찰 포인트.

- **G-1 Dockerfile Rust 버전 정책** ✅ 적용 완료 (2026-05-13)
  여러 차례 MSRV 미스매치로 빌드 실패한 이력:
  1. `1.80` → `base64ct 1.8.3`이 edition2024 요구 → 1분 31초 만에 실패
  2. `1.86` → `byoridb-kvstore/src/backup.rs:490`의 `unsigned_is_multiple_of`가 1.87 stabilize → 16분 42초 만에 실패
  3. `1.90` → 컴파일 통과(이후 `COPY config` 부재 / env Vec 파싱 이슈에서 추가 실패. 이 둘은 G-6, G-7로 분리)
  적용: Dockerfile `rust:1.90-slim-bookworm` 고정 + `rust-toolchain.toml` 추가(channel="1.90"). 로컬/CI/Docker가 동일 toolchain 사용. 후속: CI에 `rustup show && cargo check`로 toolchain 일치 검사 추가는 별건.
- **G-2 `byoridb-server` 분산 launcher 통합** (High)
  PLAN.md 검증된 기능에 분산 클러스터가 있지만, `byoridb-server` bin은 single-node only. 분산 모드를 위한 환경변수/CLI 인터페이스 부재(`AppConfig`에 peer list/cluster ID/raft 옵션 없음). 운영용 분산 배포 전에 필수. 인터페이스 예: `BYORIDB__CLUSTER__PEERS`, `BYORIDB__CLUSTER__NODE_ID`, `BYORIDB__CLUSTER__BOOTSTRAP`.
- **G-3 컨테이너 빌드 시간 단축** (Medium — redb 전환으로 우선순위 하락)
  2026-06-05 redb 전환으로 RocksDB C++ 컴파일(수 분)이 사라져 clean build ~28s.
  hot path 상당 부분 해소. 남은 후보:
  1. cargo-chef로 dependency layer 분리(workspace 외부 의존 변경 빈도 낮음)
  2. ACR Tasks `--cache-from`로 이전 이미지 layer 재사용
  3. multi-arch가 불필요하면 ACR Tasks 대신 GitHub Actions self-hosted + sccache 검토
- **G-10 배포 시 LB allowlist 클로버** ✅ 수정 완료 (2026-06-12, db09b74)
  deploy.yml의 매니페스트 재적용이 `byoridb-public`의 `loadBalancerSourceRanges`를
  매니페스트 baked IP로 매 배포마다 리셋 → 유동 IP 운영자가 배포 직후 잠김(2회 발생).
  수정: apply 전 라이브 값 저장 → apply 후 복원. `kubectl patch`가 allowlist 단일
  진실원, 매니페스트 값은 svc 최초 생성용 bootstrap default (04-services.yaml 주석 참조).
- **G-4 Azure 배포 스크립트화** ✅ 적용 완료 (2026-05-14)
  `deploy/azure/bootstrap.sh` 작성. 부딪혔던 동시성 제약을 모두 봉인:
  - AKS create는 `--attach-acr` 없이 → 별도 `az aks update --attach-acr` 단계로 분리
  - 클러스터 mutation 전후 `ProvisioningState=Succeeded`까지 polling(노드풀 add 시 `OperationNotAllowed` 차단)
  - `loadBalancerSourceRanges`는 `BYORIDB_LB_ALLOWED_CIDR` 또는 `curl ifconfig.me`로 자동 채움(S-12)
  - 멱등성: RG/VNet/ACR/AKS/이미지/노드풀/Secret 각 단계에 `exists?` 가드
- **G-5 환경변수 prefix 일관성** (Low)
  `AppConfig`는 `BYORIDB__SERVER__GRAPH_ADDR`(double underscore separator). 반면 root password는 `BYORIDB_ROOT_PASSWORD`(단일 underscore)로 `AuthManager`가 직접 `std::env::var` 호출. K8s Secret/ConfigMap에서 envFrom 사용 시 두 패턴이 섞임. `BYORIDB__AUTH__ROOT_PASSWORD` 같은 형태로 통일하면 일관성 확보(behavior breaking이라 신중).
- **G-6 `config` crate의 `Vec<String>` 환경변수 파싱 미지원** ✅ 적용 완료 (2026-05-14)
  `src/config.rs`의 `Environment` 빌더에 `prefix_separator("__")` + `separator("__")` + `try_parsing(true)` + `list_separator(",")` + `with_list_parse_key("storage.data_paths")` 추가.
  단위 테스트 4건 추가(default / single value / comma list / k8s service env 무시). K8s 매니페스트의 `unset` 해킹은 제거됨. 이제 운영 환경은 ConfigMap `BYORIDB__STORAGE__DATA_PATHS: "/data/a,/data/b"`처럼 직관적으로 지정 가능.
- **G-7 Dockerfile ENV의 sticky 동작** ✅ 적용 완료 (2026-05-14)
  Dockerfile에서 `ENV BYORIDB__*` 3개 라인 제거. 설정 출처는 (1) `AppConfig` 코드 default, (2) optional `byoridb.{toml,yaml}` 파일, (3) deploy 시 주입되는 환경변수로 일원화. 이미지에 baked되지 않으므로 K8s ConfigMap이 단일 source of truth.
- **G-8 K8s Service env auto-inject + `BYORIDB` prefix 충돌** ✅ 적용 완료 (2026-05-14)
  이중 방어로 봉인:
  1. K8s 측: StatefulSet template에 `enableServiceLinks: false`(이미 적용)
  2. 코드 측: `Environment::prefix_separator("__")`로 single-underscore env(`BYORIDB_PUBLIC_*`, `BYORIDB_ROOT_PASSWORD` 등)는 config crate가 더 이상 잡지 않음. 단위 테스트로 회귀 차단.
- **G-9 빌드 1회 비용이 크니 사전 점검 비용도 큼** (관찰)
  배포 1회 시도에서 5번 빌드/재배포 사이클 소요: Rust 1.80→1.86→1.90, `COPY config` 부재, env Vec 파싱. 각 사이클 ~20분 → 누적 비용 매우 큼. G-3(빌드 캐싱) + 사전 CI 통합 우선순위 ↑.

---

## 의사결정 가이드

**2026-05-29 방향 전환 이후: 온톨로지 DB(O 섹션)가 최우선 트랙이다.**
아래 순서로 결정:

0. **온톨로지 추론 전략(O-0)이 정해졌는가?**
   - 아니오 → **O-0 리서치 먼저**(deep-research). 전략 없이 O-5 추론 엔진
     착수 금지. 단, 전략과 무관한 선결 부채(O-1 역방향 인덱스, O-2 변길이
     경로)는 병행 착수 가능.
   - 예 → O-1 → O-2 → O-3/O-4 → O-5 순으로 진행
1. **운영 부하 / 실제 사용자가 있는가?**
   - 예 → A(운영 도구) → B(KVStore 안정성 → 성능)
   - 아니오 → 다음
2. **명확한 측정 동기가 있는가?** (특정 워크로드의 병목 식별)
   - 예 → 해당 영역(B/C/E)에서 측정 → 개선 → 재측정
   - 아니오 → 다음
3. **기능 갭이 막혀 있는가?**
   - 예 → D(정확성/기능)
   - 아니오 → **보류**가 정답

운영 부하 없는 상태에서 코드 변동을 계속 만드는 것 자체가 비용이다.
단, 온톨로지 방향(O)은 "기능 갭"이 아니라 "프로젝트 정체성"이므로 위 보류
원칙의 예외 — 적극 진행 대상이다.

---

## 완료 아카이브

기록 보존 목적. 새 작업을 시작할 때 다시 읽을 필요는 없다.

### 운영 배포 로드맵 (구 ROADMAP.md)

전 Phase ✅ 완료(2026-05 기준).

- **Phase 1 데이터 안정성**: WAL, graceful shutdown
- **Phase 2 인증/보안**: 사용자/role-based 권한
- **Phase 3 쿼리 완성도**: WHERE 절, edge CRUD
- **Phase 4 모니터링**: Prometheus 메트릭, JSON 구조화 로그
- **Phase 5 분산 시스템**: 파티셔닝, Raft 복제, 인덱스
- **Phase 6 운영 보강**: 부하 테스트(31K QPS), 장애 복구, 백업/복구
- **Phase 7 분산 핵심**: Raft 네트워크/스토리지/스냅샷/멤버십, Meta Client/Server
- **Phase 9 코드 품질**: executor.rs 함수 분리, parser 모듈화, key 인코딩 통합
- **Phase 10 수평 확장**: HASH/RANGE/MODULO 파티셔닝, 분산 스토리지/쿼리/인덱스, proto 직렬화(JSON 대비 30–50% 절감)

### Mock/Hardcoded 청산 (구 MOCK_REMEDIATION_PLAN.md)

PR 1–10 모두 ✅. 잔여는 위 "남은 작업 D"에 흡수.

### INSERT EDGE edge_type 하드코딩 후속 (2026-05-14)

PR 1~10 Mock/Hardcoded 청산에서 누락됐던 항목. `byoridb-executor/src/plan.rs:600`에 `edge_type: 0 // would be resolved from schema` 주석과 함께 박혀 있던 초기 커밋(`1572c31`) 코드. 모든 INSERT EDGE가 schema lookup 시 "Edge not found: 0"으로 실패해 데이터 삽입 자체가 막혀 있었음. S-4 INSERT/UPDATE 스키마 검증이 ✅로 표시되어 있었지만 validation 이전 단계에서 lookup이 항상 실패해 검증 의도가 무효화된 상태.

수정: `EdgeInsert.edge_type: i32` → `String`, parser의 `EdgeInsertSpec.edge_name`을 그대로 전달. 회귀 테스트 2건 추가(`plan::tests::insert_edge_statement_carries_edge_name_into_plan`, `insert_edge_multi_row_preserves_edge_name_for_each_row`). 단위 테스트가 못 잡았던 이유는 기존 helper `insert_test_edge`가 kvstore에 직접 쓰는 방식이라 buggy plan 경로를 우회.

### 보안 강화 (S-1~S-14, 2026-05-13)

심층 코드 분석(2026-05-13)에서 발견된 Critical/High/Medium 이슈 수정. S-12(TLS)만 미완.

| ID | 항목 | 상태 |
|---|---|---|
| S-1 | RBAC check_permission 실제 호출 + Statement→Permission 매핑 | ✅ |
| S-2 | 세션 ID AtomicI64 → OsRng random i64 | ✅ |
| S-3 | AUTH-SYNC write-through (CREATE USER/GRANT/REVOKE → AuthManager) | ✅ |
| S-4 | INSERT/UPDATE 스키마 검증 (tag/edge 존재 + field 이름) | ✅ |
| S-5 | scan_prefix limit(100K) + GO step 상한(20) | ✅ |
| S-6 | set_null_flag panic (nullable 0개 스키마) | ✅ |
| S-7 | delete_session 소유권 검증 | ✅ |
| S-8 | WAL checksum wrapping_add → CRC32C (crc32fast) | ✅ |
| S-9 | gRPC max_decoding_message_size=64MB | ✅ |
| S-10 | 백업 디렉토리 0o700 권한 | ✅ |
| S-11 | Meta HTTP 기본 바인딩 0.0.0.0 → 127.0.0.1 | ✅ |
| S-12 | TLS 활성화 | 의도적 미구현 — 네트워크 격리로 대체 (운영 시 VPC/방화벽 필수) |
| S-13 | RocksDB write_buffer_size=64MB, max_write_buffer_number=3 | ✅ |
| S-14 | auth.rs 해싱 중복 제거, DataType fallback warn, Geography bounds check | ✅ |

**워크스페이스 테스트**: 517개 통과 (S-15 brute-force 테스트 2개 추가).

| ID | 항목 | 상태 |
|---|---|---|
| S-15 | Brute-force 방어 (5회 실패 → 5분 잠금) | ✅ |
| S-16 | Heartbeat cluster_id 검증 | ✅ |
| S-17 | 세션 sliding window (활동 시 expires_at 연장) | ✅ |
| S-18 | HTTP 쿼리 문자열 1MiB 제한 | ✅ |

| PR | 항목 | 커밋 |
|---|---|---|
| 1 | CREATE/DROP/SHOW TAG/EDGE INDEX 연결 | `248408d` |
| 2 | DEFAULT value, MATCH `{k:v}` 필터, matches_node/edge | `a1752fe` |
| 3 | SHOW HOSTS/PARTS 하드코딩 제거 | `22c8002` |
| 4 | gRPC latency, lexer 라인번호, version 삭제 | `9500c5e` |
| 5 | Datetime epoch silent, host fallback warn, heartbeat debug | 누적 |
| 6 | LOOKUP `part_id=1` 하드코딩 제거 | `0187e51` |
| 7 | `$var = ...; GO FROM $var` compound statement | `9a19bfd` |
| 8 | 클라이언트 인증 하드코딩 제거 | `7701033` |
| 9 | gRPC execute 응답 proto `DataSet` 구조화 | `75edfab` |
| 10 | BFS edge_type 매칭, MATCH interior node 필터 | `1d90124`(흡수) |

### 그래프 알고리즘 최적화 (구 GRAPH_ALGORITHM_OPTIMIZATION_PLAN.md)

Phase 0–7 모두 ✅, 커밋 `1d90124`. 핵심 임팩트:

- **Phase 2**: `get_neighbors` 공통 helper, proto/JSON 자동 디코딩 일원화
- **Phase 3**: `KVStore::scan_stream` BoxStream + RocksDB channel-driven iterator, BFS `decode_edge_dst` 핫패스 → **BFS −39~49%**, star_hub 16k **−27%**, scale_free **−27%**
- **Phase 4**: Dijkstra executor 연결, `TraversalMetrics`, configurable `max_traversal_nodes`
- **Phase 5**: full-cover → prefix-cover → single-field tag index 매처, fallback 로깅
- **Phase 6**: `GetNeighborsBySource` RPC(O(degree) targeted) — 분산 GO 알고리즘 복잡도 자체 개선
- **Phase 7**: criterion harness + chain/star/scale-free 생성기

### KVStore 성능 (구 NEXT_STEPS.md)

- **단위 테스트 보강**: byoridb-common, byoridb-codec, byoridb-parser, byoridb-kvstore(8→82개 +74).
- **WAL 가설 #1 패치**: `write_entry::flush` 제거 → `single_put/wal_kvstore` −33.4%, `hundred_puts/serial` −33.7%. 외부 WAL은 buffered 시맨틱.

---

## 측정 환경

- 워크스페이스 빌드: `cargo build --workspace`
- 워크스페이스 테스트: `cargo test --workspace --lib`
- KVStore 벤치: `cargo bench -p byoridb-kvstore --bench wal_overhead`
- 그래프 알고리즘 벤치: `cargo bench -p byoridb-executor --bench graph_traversal`
- CRAP 측정: `scripts/crap_check.sh` + `scripts/crap_analyze.py`

기준 환경: macOS arm64, criterion 0.5, release profile.

---

## 작업 패턴 (함정 방지)

이전 세션에서 마주친 실수들 — 다음 작업에서도 동일하게 적용됨.

**테스트 작성**

- WAL 체크섬 깨짐 테스트는 *데이터 영역*(키/값 바이트)만 flip. 구조 필드(`key_len`/`value_len`) flip 시 truncate 에러로 갈라짐.
- `WalKVStore` 테스트는 `#[tokio::test(flavor = "multi_thread")]` 필수. current_thread에서 `block_in_place` panic.
- backup ID는 Unix-second timestamp — 같은 초 내 다중 backup 디렉토리 충돌, 1초 sleep 회피.

**성능 측정**

- criterion 벤치는 매 iter마다 새 키 사용(LSM 누적 회피). `AtomicU64` monotonic counter.
- 디스크 I/O 벤치 `sample_size` 30~50 권장. 100은 시간 낭비.
- *통제군* 반드시 같이 측정(예: memory backend, batch path). 의도치 않은 영향 확인용.
- `change p > 0.05`는 noise. 기준선 대비 Δ가 5σ 이상일 때만 의미 있음.

**커밋/푸시**

- pre-commit 훅이 `cargo fmt` 검사 — 커밋 전 `cargo fmt --all` 선실행.
- 커밋 메시지: `<type>(<scope>): <subject>`. type은 feat/fix/refactor/chore/docs.
- `.claude/`는 로컬 전용. 절대 커밋 금지.
