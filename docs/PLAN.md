# ByoriDB 플랜

마지막 업데이트: 2026-06-22 (온톨로지 핵심 O-1~O-9 + 유사도 추천 R-1~R-3b 구현 완료.
O-8 owl:sameAs(write-time canonical merge)·O-9 삭제 retraction(full re-materialization)
구현·검증 완료. O-8 배포됨, O-9 미배포. 상세는 O/R 섹션. 남은 건 retraction B/F 최적화·
분산 materialization·운영.)

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

**진척 (2026-06-17 갱신):** 온톨로지 핵심이 구현·배포 완료됐다 — 클래스 계층(O-3),
시맨틱 관계 타입(O-4), 추론 엔진(O-5: edge materialization + domain/range vertex
타입 추론), 일관성 검사(O-6: disjoint), 시맨틱 쿼리(O-7: `is_a`). 별도 차별화
트랙으로 유사도 추천(R-1~R-3b: 구조/임베딩 flat·HNSW/하이브리드)도 완료. 남은
온톨로지 작업은 고급 추론(`sameAs` 동치, 삭제 retraction=B/F, 분산 materialization)
뿐 — 각각 난이도·리스크 최상이라 별도 결정 필요. 상세·미착수는 **O/R 섹션** 참조.

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

**O-3 [P1] 클래스 계층 / TBox 모델링** ✅ 구현·배포 완료 (2026-06-12, 커밋
8c6989a → AKS sha-8c6989a. 프로덕션 스모크 7/7: 계층 생성/SHOW/DESCRIBE/
자기참조 거부/RESTRICT/DROP TAG 거부/INSERT+MATCH 호환, 일회용 space 정리)
신규 `executor/class_ddl.rs` (515 LoC, 테스트 포함). 파서 4 + 실행기 11 회귀,
워크스페이스 641 통과, 게이트 차단 0 (info 1건 — corrupt 메타 무음 폐기 →
에러 전파로 즉시 해소: DROP RESTRICT 자식 검사가 전 자식을 봐야 함).
ALTER CLASS(SUBCLASS 변경)·분산 meta 연동(G-2 이후)·추론 포함 매칭(O-5/O-7)은
후속. 설계 결정(D1~D6)은 아래 유지.
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

**O-4 [P1] 시맨틱 관계 타입** ✅ 구현 완료 (2026-06-17)
edge-type에 시맨틱 관계를 1급 메타데이터로: `CREATE EDGE <e>(...) [TRANSITIVE]
[SYMMETRIC] [INVERSE OF <e>] [SUBPROPERTY OF <e>]`. 시맨틱 플래그를 edge 스키마
JSON(`space:{space}:edge:{name}` → `"semantics": {...}`)에 저장. AST `SemanticFlags`,
신규 토큰 TRANSITIVE/SYMMETRIC/INVERSE/SUBPROPERTY(+OF 재사용). CREATE 시 INVERSE
OF/SUBPROPERTY OF 대상 edge 존재 검증 + 자기참조 거부. `subClassOf`는 O-3에서
이미 제공. `domain`/`range`(vertex 타이핑)·`sameAs`는 후속(ABox 타입 모델/최난도).
회귀: 파서 1(DDL 시맨틱 파싱) + 실행기 검증.

**O-5 [P1] 추론 엔진 (O-0 결정 반영)** 🟡 phase 1 (edge-level, insertion-only) 완료 (2026-06-17)
O-0 결정대로 **RDFS-Plus forward-chaining materialization**. 신규 `executor/
inference.rs`. INSERT EDGE가 커밋 후 entailed edge를 **fixpoint(worklist)까지** 도출해
같은 `{space}:edge:`/`in-edge:` 키스페이스에 `__inferred__` 마킹·ranking 0으로 저장
→ MATCH/GO가 추론 edge를 질의타임 추론 없이 그대로 봄(end-to-end 테스트로 확인).
- **규칙(edge-level)**: symmetric, inverseOf(양방향), subPropertyOf, transitive.
  cascading 완전 폐포(예: subPropertyOf→transitive 초프로퍼티). `RelMeta` 인덱스
  로드 + 증분 worklist(seeds=삽입 triple, 기존 그래프와 결합). `max_traversal_nodes`
  write 캡(pathological closure 가드, warn).
- **scope**: **insertion-only**(삭제 retraction은 후속 B/F 알고리즘). 시맨틱은
  INSERT 전에 선언(`CREATE EDGE ... TRANSITIVE` → `INSERT`). 기존 데이터에 시맨틱
  후행 추가 시 재-materialization 필요(미구현, O-1 백필 caveat과 동일 성격).

**O-5 phase 2: domain/range → vertex 타입 추론 ✅ 구현 완료 (2026-06-17)**
`CREATE EDGE <e>(...) [DOMAIN <class>] [RANGE <class>]`. `(a)-p->(b)`에서 p domain
C ⟹ a is-a C, p range D ⟹ b is-a D. 추론 타입을 **`{space}:vtype:{vid}:{class}`**
키스페이스에 저장(정점 blob 미변경). `ontology::vertex_class_set`가 vtype를 소비 +
조상클래스 확장 → `is_a`(MATCH/RECOMMEND)가 추론 타입을 자동 인지. 신규 토큰
DOMAIN/RANGE(RANGE는 PARTITION BY RANGE와 공존 — 파서 양쪽 처리). materialization
worklist가 처리하는 매 triple(asserted+inferred edge)에 domain/range 적용 → 추론
edge에도 타입 전파. CREATE 시 domain/range 대상 class(tag/class) 존재 검증.
회귀: domain/range 타입 추론(서브클래스 조상 + MATCH is_a 가시) + 검증 + 파서.
- **미착수(후속)**: `sameAs`(최난도), 삭제 증분(B/F), 분산 materialization(먼 마일스톤).
- 회귀 누적: 규칙(symmetric/inverseOf 양방향/subPropertyOf/transitive/cascading/
  no-semantics/순회 가시성) + domain/range + MATCH is_a.
- **후속 최적화(리뷰 F-001, 비차단)**: 시맨틱 미선언 space도 INSERT EDGE마다
  `load_rel_meta`가 edge 스키마 프리픽스를 1회 스캔(비용은 edge *타입* 수에 비례,
  보통 수십 이하). bidirectional inverse 정확성상 전체 스캔이 필요 — 핫패스 최적화
  필요 시 RelMeta를 space별 캐시(CREATE/ALTER EDGE에서 무효화)로 개선.

**O-6 [P2] 일관성 검사 (consistency / validation)** ✅ disjoint 검사 구현 완료 (2026-06-17)
`CHECK CONSISTENCY` → disjoint 클래스 위반(한 정점이 서로소 선언된 두 클래스에 동시
소속) 탐지. `CREATE CLASS x() [DISJOINT WITH c1[, c2]]`로 선언(대칭, 클래스 메타
`disjoint`에 저장, 대상 존재+자기참조 검증). 신규 `executor/consistency.rs`:
모든 클래스의 disjoint 맵(대칭) 구성 → 정점 스캔하며 `vertex_class_set`(tags ∪
O-5 추론타입 ∪ O-3 조상)에서 disjoint 쌍 동시 소속 시 위반 보고. **subclass·
domain/range 추론으로 생긴 간접 위반도 탐지**. 결과 컬럼 vid/class_a/class_b
(쌍 정렬 dedup), 빈 결과 = 일관됨. 신규 토큰 DISJOINT/CHECK/CONSISTENCY + 신규
`Statement::CheckConsistency` 전 경로(파서/plan/executor/graph RBAC=Read).
회귀: disjoint 위반(추론타입 경유)·clean·DDL 검증·파서.
- **open-world 주의**: domain/range는 *추론적*(타입 추가)이라 위반 아님 — disjoint만
  검사. SHACL식 cardinality/closed-world 제약은 별도(미착수).

**O-7 [P2] 시맨틱 쿼리 표면** 🟡 `is_a` 매칭 구현 완료 (2026-06-17)
nGQL에서 추론/클래스 계층 인지 쿼리 노출. **`MATCH (n:dog) WHERE is_a(n, "animal")
RETURN n`** — 후보 정점의 tag가 해당 클래스이거나 그 subclass(O-3 계층)면 매칭.
RECOMMEND WHERE의 `is_a`(R-3b)를 주 질의 언어 MATCH까지 확장. 신규 공유 모듈
`src/ontology.rs`(`class_ancestors_of` + `vertex_class_set`)로 RECOMMEND·DESCRIBE
CLASS·MATCH가 한 구현 공유(class_ddl::class_ancestors도 위임). match_executor의
eval_return_expr에 2-arg `is_a(<var>, "<class>")` 함수 + eval_condition에 bool-valued
FunctionCall 처리 추가. 회귀: MATCH is_a(subclass 매칭/cat 제외/negative).
- **미착수(후속)**: 추론 edge 기반 매칭(이미 inferred edge가 GO/MATCH에 보이므로
  일부 자동 충족), SPARQL 호환(보류), `MATCH (n:animal)` label 자체의 subclass
  확장(현재는 label 정확 매칭 + WHERE is_a로 해결).

**O-8 [P1] owl:sameAs 노드 동치 추론 (write-time canonical merge)** ✅ 구현·검증
완료 (2026-06-19), 2026-06-22 main 머지 → AKS 자동배포

O-0이 "가장 비싼 outlier라 마지막"으로 미룬 동치 추론. **2026-06-19 deep-research**
(25 claim 전부 3-0 confirmed): sameAs를 congruence relation으로 full materialize하면
조합 폭발(크기 n 클래스 → n² triple을 2n³ derivation; 실측 3,930-member = 600억
derivation·4h+). 모든 production RDF store(GraphDB·RDFox·Stardog·Oracle)의 표준은
**canonical-representative rewriting**(대표 노드 UnionFind + 쿼리타임 expansion,
triple 7.8×·시간 31.1× 절감). LPG 선례는 전무(Neo4j n10s = sameAs 미지원). retraction이
결정적 난제(2015 B/F≈까지 미해결) — insertion-only는 merge 비가역.

*설계 결정 (D1~D10, 사용자 = write-time canonical merge 선택):*
- **D1. 예약 edge type `sameAs`.** `CREATE EDGE sameAs()` → `INSERT EDGE sameAs()
  VALUES a->b:()`. instance-level이라 owl 의미론에 맞고 parser 무변경. O-4 시맨틱
  플래그 붙이지 말 것(merge 전담, O-5 규칙 대상 아님).
- **D2. 대표 = min vertex ID, UnionFind.** 사이드스토어 `{space}:sameas:{vid}`→대표
  (i64 LE), `{space}:sameas-members:{rep}:{member}`→역방향 멤버 열거. `sameas-`
  infix로 두 키스페이스 분리.
- **D3. write-time merge.** 비대표 loser→대표 winner로 out/in-edge(정·역 인덱스
  동기), vtype, tagvid(블롭 tags로 키 재구성), vec dense(dirty 마킹), vertex blob
  rewrite 후 loser 키 삭제. self-loop는 winner→winner로.
- **D4. 비가역·원본 미보존(lossy).** insertion-only이므로 retraction 미지원. 대표에만
  fact를 남겨 읽기는 입력 정규화만(저비용). **trade-off: 동치 오선언 시 되돌리기
  불가** → D7 가드.
- **D5. 읽기 = 입력 vid 정규화(expand 아님).** GO `from_vids`(execute_go_local)·FETCH
  `resolved_vids`·MATCH `id(n)==X` 바인딩을 신규 `ontology::representative_of`로 대표
  치환. merge로 비대표 blob이 삭제되므로 MATCH 후보 스캔은 자동 suppress(명시 필터
  불요). O-5 materialization 내부 algo는 raw vid 유지(정규화는 진입점만).
- **D6. 속성 충돌 = 대표(min-id) 우선.** loser prop은 winner에 없는 것만 추가.
- **D7. DELETE 가드.** merged 노드(비대표 멤버 or 멤버 가진 대표) DELETE VERTEX 거부,
  sameAs DELETE EDGE 거부, DROP EDGE sameAs 거부 — 전부 `InvalidOperation`.
- **D8. read-side 폭발 방어.** 입력 정규화라 결과가 대표 1개로 수렴(Stardog식 자동).
- **D9. write cap.** merge 횟수에 `max_traversal_nodes` cap(O-5 패턴, warn).
- **D10. O-5 순서.** INSERT EDGE 직후 **merge 먼저** → 그 다음 O-5 materialization
  (canonicalized 그래프 위 추론). sameAs triple은 O-5에 안 넘김.

*구현:* 신규 `executor/sameas.rs`(merge 엔진, `merge_sameas_triples`+rewrite 헬퍼) +
`ontology.rs`(`representative_of`/`members_of`/`encode_repr`) + `key.rs`(sameas/tagvid
빌더). 훅: `dml.rs` INSERT EDGE(merge→필터→materialize)·DELETE 가드, `dql.rs`
GO/FETCH 정규화, `match_executor.rs` id() 정규화, `ddl.rs` DROP EDGE 가드. 회귀:
sameas 유닛 6(대표선출/out·in-edge rewrite·역인덱스/속성충돌/idempotent·union/self-loop)
+ key 1 + integration 1(end-to-end merge·GO/FETCH 정규화·3종 DELETE 거부). 워크스페이스
762 lib + 46 integration 통과, fmt·clippy 클린.
- **프로덕션 스모크 ✅ (2026-06-22)**: AKS(`20.249.128.19`, sha-13d3e70)에서 merge·
  GO/FETCH 정규화·DELETE 3종 거부 전부 기대대로 통과(HTTP API).
- **RECOMMEND × sameAs 연계 ✅ (2026-06-22)**: `execute_recommend`가 시드 vid를
  `representative_of`로 정규화(GO/FETCH/MATCH와 일관, D5). merge가 후보(edge/vec)를
  이미 대표로 collapse하므로 후보·결과 정규화는 불필요(자동). 유사도로 후보를 *발견*→
  sameAs로 *단언*하는 entity resolution 루프의 역방향 완성. 회귀: recommend 유닛 1
  (merged-away 시드가 대표 임베딩으로 추천).
- **후속**: 삭제 retraction → **O-9에서 1단계 완료**, 분산 materialization(G-2 선결).

**O-9 [P1] 삭제 retraction (full re-materialization)** ✅ 구현·검증 완료 (2026-06-22,
미배포)

O-5 추론이 insertion-only라 DELETE EDGE/VERTEX가 stale 추론을 남기던 한계 해소
(예: `ancestor` TRANSITIVE에서 1→2,2→3 ⟹ 1→3 inferred인데 2→3 삭제해도 1→3 잔존).
O-0이 정한 **"1단계 full re-materialization → 2단계 B/F, DRed 회피"** 중 사용자가
1단계 선택.

*설계 (D1~D5):*
- **D1. 트리거 = DELETE EDGE/VERTEX 후, 시맨틱 선언 space만.** `load_rel_meta` empty면
  스캔 전 조기 no-op → 시맨틱 미사용 space는 비용 0·무회귀(INSERT 대칭).
- **D2. full re-mat (기존 inference.rs 재활용).** 신규 `Executor::rematerialize_space`:
  `{space}:edge:` 스캔 → `__inferred__` edge는 정·역 삭제, asserted는 seed 수집 →
  `{space}:vtype:` 전부 삭제 → `run_materialization(asserted)` 재실행.
- **D3. 멱등·완전.** inferred 전부 폐기 후 asserted로 재도출 → overdeletion/잔존 없음
  (DRed 회피 이점). 다른 경로로도 도출되는 inferred(또는 asserted 중복)는 재도출되어 유지.
- **D4. sameAs 상호작용 없음.** asserted edge·vtype은 이미 대표로 rewrite돼 있어 재mat가
  대표 기준 재도출. sameAs 맵(`{space}:sameas:`)은 비가역이라 불변.
- **D5. 비용 = O(graph)/delete** (시맨틱 space만). O-0 인정 1단계. B/F(2단계)는 후속.
- 회귀(inference.rs 5): transitive/symmetric retract, 다른 경로 지원 시 보존,
  domain/range vtype retract, 시맨틱 미선언 no-op. executor lib 187 + integration 46 통과.
- **후속**: B/F 증분(O-0 2단계, deep-research 선행), DELETE VERTEX edge cascade(별개 이슈).

### R. 유사도 / 추천 (P1 — 차별화 기능, 2026-06-15 신설)

"어떤 노드와 가장 유사한 노드 top-k 추천" — 채널 간 동일 상품 후보 발견
(entity resolution) 같은 use case. **O-시리즈(논리적 추론)와 다른 축**: 규칙으로
A=B를 *단언*하는 대신, 가까운 후보를 *발견*한다. 실질적으로 O-0이 "가장 비싸서
마지막으로 미룬" `owl:sameAs` 동치를 우회하는 실용 경로. **하이브리드(벡터+그래프+
온톨로지)가 최종 목표이며, 구조→벡터→온톨로지 결합 순으로 단계 구축.**

**R-1 [P1] 구조적 유사도 (Jaccard, 무-임베딩)** ✅ 구현 완료 (2026-06-15)
`RECOMMEND SIMILAR TO <vid> OVER <edges>|* [LIMIT k]`. 공유 이웃 겹침으로
유사도 계산: `sim(a,b) = |N(a)∩N(b)| / |N(a)∪N(b)|`. N(v)=v의 out-neighbor
집합(edge 타입 필터, `*`=전체). 신규 `executor/recommend.rs`(executor.rs 비대화
회피). **후보 생성**은 시드의 각 이웃을 O-1 역방향 인덱스로 거슬러 올라가
공유 이웃 ≥1인 정점만 수집(전체 스캔 회피, `max_traversal_nodes` 캡). 결과
컬럼 `vid/score/shared`, 랭킹 score desc→shared desc→vid asc.
- 토큰 RECOMMEND/SIMILAR(2개), AST `RecommendStatement`+`SimilarityMetric`,
  plan `RecommendPlan`, graph 서비스 RBAC(Read)+QueryType(Recommend) 매핑,
  EXPLAIN plan_kind arm 추가.
- 회귀: 파서 4(기본/다중edge·기본limit/`*`/limit 0 거부) + 실행기 4(Jaccard
  랭킹·0겹침 제외/limit truncate/이웃 없는 시드 빈결과+스키마/edge 타입 스코프).
  워크스페이스 전체 통과, fmt·clippy(변경 크레이트) 클린.
- **caveat**: 후보 생성이 역방향 인덱스 의존 → O-1 이전 로드된 space는 재로드
  필요(reverse GO/BIDIRECT와 동일). 원시 텍스트 차이(제목 표기 차이)는 못 잡음
  → 공유 속성 노드로 정규화돼 있어야 동작. 그래서 R-2 필요.
- 미착수: 배포·프로덕션 스모크, 분산 meta 연동(G-2 이후).

**R-2 [P1] 벡터 임베딩 유사도 (ANN)** 🟡 R-2a 구현 완료 (2026-06-16), R-2b 미착수
정점에 임베딩 벡터 속성 저장 + 코사인/L2 최근접 이웃 검색. 채널마다 제목이
달라도 의미가 가까우면 매칭 → 사용자 핵심 use case 해결. **R-1(구조적)이 못 잡는
원시 텍스트 차이를 푸는 단계.**

**리서치 확정 (2026-06-16):** 순수 Rust ANN 크레이트 `instant-distance 0.6.1`,
`hnsw_rs 0.3.4` 둘 다 **Rust 1.90 클린 컴파일 실측 완료** (openraft/validit 같은
landmine 없음). R-2b는 `instant-distance`(더 가벼움, serde 영속화) 우선.

*설계 결정 (2026-06-16):*
- **D1. 벡터 타입.** 신규 `Value::Vector(Vec<f32>)` (List<Float(f64)> 재사용
  기각 — f32로 메모리 절반/타입 안전/차원 검증). `byoridb_common::Value` +
  codec `PropertyType` + meta `DataType`에 각각 `Vector(dim)` variant 추가.
  **Value enum 확장은 blast radius 큼** — type_of/accessor/PartialEq/serde/
  to_string 전부 갱신 필요. (Value::PartialEq가 List/Map 등에서 무조건 false였던
  O-2 버그 선례 주의 — Vector도 구조 비교 명시.)
- **D2. 임베딩 생성은 DB 밖.** 클라이언트가 외부 모델로 만든 f32 벡터를
  INSERT VERTEX 속성으로 전달. DB는 저장·검색만(ML 모델 미내장). *확정.*
- **D3. 거리.** 코사인(기본, 정규화 텍스트 임베딩) + L2 옵션, 인덱스별 선언.
- **D4. 단계 분할 (flat 먼저 — 2026-06-16 결정).**
  - **R-2a flat 정확 KNN** ✅ 구현 완료 (2026-06-16). 의존성 0. dense f32
    사이드 스토어 prefix scan → 코사인 → top-k. 수만 벡터까지 정확·충분.
    R-1과 같은 "정확한 것 먼저" 규율.
  - **R-2b HNSW 근사** ✅ 구현 완료 (2026-06-17). 영속 인덱스 방식(사용자
    go + D9-A 선택). 대규모 차별화. R-2a dense 사이드 스토어 위에 ANN 레이어.
    신규 `executor/vector_index.rs` + 의존성 `instant-distance 0.6`(with-serde).

    *구현 (2026-06-17):*
    - **영속 인덱스**: `{space}:vecidx:{prop}` → `bincode(HnswMap<Emb,vid>)`.
      HnswMap이 point→vid 매핑 내장 → 검색이 vid 직접 반환. 커스텀 `Emb` Point가
      코사인 distance(`1-cos`). `{space}:vecidx-dirty:{prop}` 마커로 stale 추적.
    - **임계값**: `config.vector_index_min`(기본 1000) 이하면 인덱스 없이 exact
      flat KNN(R-2a) — instant-distance가 build-once(증분 insert 없음)라 rebuild가
      O(N)이므로, 대규모 카탈로그(벌크 적재→다수 쿼리)에서만 인덱스가 이득.
      깨끗한 인덱스 쿼리는 풀스캔 0(load+search). dirty/없음 → 1회 재빌드.
    - **쿼리 표면 무변경**: `BY EMBEDDING`이 내부에서 인덱스 유무로 ANN/flat 분기.
      flat은 always-correct 폴백·검증 기준. ANN은 근사(recommendation 허용).
    - **무효화**: INSERT/UPDATE의 숫자-리스트 prop → `mark_vector_index_dirty`.
    - **DELETE 정리 (후속 완료 2026-06-17):** DELETE VERTEX가 삭제 정점을 디코드해
      숫자-리스트 prop의 dense 엔트리를 삭제 + 해당 인덱스 dirty 마킹 → 다음 쿼리
      재빌드가 삭제 점을 제외(tombstone 누적·recall 저하 caveat 해소). 방출 시
      정점 존재확인은 belt-and-suspenders로 유지. (이전 caveat 제거됨.)
    - **ANN budget LIMIT 연동 (후속 완료 2026-06-17):** 고정 256 폐기 →
      `BY EMBEDDING`이 LIMIT·필터 유무로 over-fetch budget 산출(필터 시 ×16, 무필터
      LIMIT+32), `ANN_SEARCH_BUDGET_MAX=4096` 상한. heavy WHERE+큰 LIMIT under-return 완화.
    - **회귀**: vector_index 3(ANN 영속·랭킹 / dirty 재빌드 / 임계값 flat) +
      recommend DELETE 정리 1. 기존 임베딩 12건 flat 유지(무회귀).
    - **D8 종결 (2026-06-17)**: hnsw_rs(증분 insert) vs instant-distance 택일 →
      **instant-distance로 종결.** 사유: 사용자가 고른 D9-A(redb-bytes 직렬화)와
      hnsw_rs의 **파일 기반 영속이 근본 충돌** — hnsw_rs 도입은 D9-A 결정을 되돌리는
      것. 현재 영속+dirty 재빌드로 동작하며 rebuild-on-write(O(N))는 벌크적재→다수
      쿼리 워크로드에서 수용 가능. 쓰기가 실제 병목이 되면 그때 별도 트랙으로
      재평가(아키텍처 변경 동반이라 자율 전환 대상 아님).

    *설계 결정 (2026-06-16):*
    - **D8. 크레이트.** 증분 insert가 DB에 중요 → `hnsw_rs 0.3.4`(insert +
      file dump/load 지원)가 `instant-distance 0.6.1`(build-once, serde)보다
      적합 가능성. 단 hnsw_rs는 deps 무거움(mmap-rs/sysctl/cpu-time). 둘 다
      1.90 컴파일 실측 OK. **구현 시 instant-distance의 증분 insert 한계
      재확인 후 택1** (build-once면 쓰기마다 재빌드 비용).
    - **D9. 인덱스 생명주기 (핵심 fork).**
      - (A) **영속 인덱스**: HNSW를 redb에 직렬화, 시작 시 로드 + INSERT/UPDATE
        증분 insert + DELETE tombstone. 정확·빠르나 **새 온디스크 포맷 + 증분
        유지 복잡도**.
      - (B) **lazy in-memory 재빌드**: dense 스토어에서 첫 쿼리 시 빌드·캐시
        (OnceCell/RwLock), 쓰기 시 무효화. 온디스크 포맷 0, 단순하나 쓰기 후
        첫 쿼리에 재빌드 비용 + 캐시 무효화 정합성 주의. **권장: (B)부터**
        (dense 스토어가 source of truth 유지, R-2a 폴백 자명).
    - **D10. 정확성/폴백.** HNSW는 근사 → flat(R-2a)을 always-correct 폴백·
      검증 기준으로 유지. 인덱스 없거나 N 작으면 flat 사용(임계값). 쿼리 표면은
      R-2a와 동일(`BY EMBEDDING`), 내부에서 인덱스 유무로 분기 — 사용자 무변경.
    - **D11. staleness.** (B) 채택 시 INSERT/UPDATE/DELETE가 해당 prop 인덱스
      캐시를 무효화(다음 쿼리 재빌드). dense 스토어는 R-2a 픽스대로 항상 정합.

*R-2a 구현 메모 (2026-06-16, 커밋 예정):*
- **D1 조정**: blast radius 통제를 위해 `Value::Vector` 신규 variant는 **보류**
  (Value enum은 type_of/PartialEq/serde/codec/JSON 등 전역 exhaustive match
  20+곳 — prod 자동배포 리스크). 대신 임베딩은 **`Value::List<Float>` 속성으로
  재사용**, **dense f32 사이드 스토어**(`{space}:vec:{prop}:{vid}` → packed LE
  f32, `SchemaKey::vec_data`)가 INSERT 시 함께 기록돼 성능(메모리·스캔)을 담당.
  INSERT VERTEX가 **모든 숫자-리스트 속성**을 자동 미러링(`recommend::pack_embedding`).
- **신규 파싱**: `[...]` 리스트 리터럴(expr.rs) + `expr_to_value`의 List·단항
  Neg folding(음수 임베딩값). `BY`/`EMBEDDING` 토큰(EMBEDDING은 keyword_to_string에
  등록해 속성명으로도 사용 가능).
- **쿼리**: `RECOMMEND SIMILAR TO <vid> BY EMBEDDING <prop> [LIMIT k]` (D6 vid 기준).
  결과 컬럼 vid/score(코사인). 시드 벡터 없으면 빈결과.
- **성능**: 스캔은 packed f32만 읽음(정점 디코드 회피), 시드 norm 1회 선계산,
  단일 패스 dot+norm. 정점 존재 확인은 방출 top-k에만(stale 삭제 거름, k회 get).
- **UPDATE 일관성** (리뷰 F-001 수정): UPDATE VERTEX도 갱신된 숫자-리스트 속성을
  dense 스토어에 재미러링(리스트→비리스트면 dense 삭제). 없으면 KMM이 옛 벡터로
  조용히 채점(정점 살아있어 존재확인 통과 못 함). 회귀 테스트 추가.
- **DELETE 정리 (후속 완료 2026-06-17)**: DELETE VERTEX가 삭제 정점을 디코드해
  dense 엔트리 삭제 + 인덱스 dirty 마킹(이전의 "dense 미정리→stale" caveat 해소).
  방출 top-k 존재 확인은 belt-and-suspenders로 유지. 차원 불일치/제로벡터 스킵.
  dense 미러는 숫자 리스트 전부 대상(비-임베딩 숫자 리스트도 저장될 수 있음, 낭비 허용).
- **알려진 한계** (리뷰 F-002): dense 키가 `{space}:vec:{prop}:{vid}`로 tag 미포함
  → 한 vid의 두 tag가 동명 숫자-리스트 prop을 가지면 마지막이 덮어씀(단일-tag
  임베딩 패턴에선 무해). 다중 tag 동명 벡터 지원 시 키에 tag 포함 검토.
- **회귀**: 파서 9(임베딩 파싱·리스트 리터럴·빈리스트·모드 거부 등) + 실행기 9
  (Jaccard 3 + 임베딩 코사인 랭킹/limit·차원불일치/stale 삭제/시드없음 + pack/unpack
  round-trip + UPDATE 재미러링) + plan 1(List·단항Neg folding). 워크스페이스 통과.
- **D5. 인덱스 메타.** `CREATE VECTOR INDEX <name> ON <tag>(<prop>) DIM <n>
  METRIC cosine`. `IndexType::Vector` + dimension + metric을 `IndexDef`
  (byoridb-storage/src/index.rs)에 추가, `__meta:index_def:` JSON 패턴 재사용.
  R-2a는 인덱스 없이도 동작(flat scan), 인덱스는 R-2b 가속용.
- **D6. 쿼리 표면 (vid 기준만 먼저 — 2026-06-16 결정).**
  `RECOMMEND SIMILAR TO <vid> BY EMBEDDING [METRIC cosine] [LIMIT k]` — 존재
  정점의 저장된 벡터를 쿼리로 써서 가장 가까운 다른 정점 top-k. R-1 RECOMMEND
  동사 재사용(`BY EMBEDDING` vs R-1 기본 Jaccard). 원시 벡터리터럴 쿼리
  (`NEAREST TO <vec>`, 텍스트 검색용)는 후속.
- **D7. R-3 연계.** WHERE 절로 그래프/온톨로지 제약(다른 채널, O-3 같은 클래스)
  → ANN 후보 + 그래프 필터 = 하이브리드(R-3).

**R-3 [P1] 하이브리드 온톨로지 인지 추천** 🟡 R-3a 구현 완료 (2026-06-16)
R-2 ANN으로 후보 → 그래프/온톨로지 제약으로 필터·재랭킹(다른 채널 한정, O-3
클래스 계층상 같은 상위 카테고리, 공유 브랜드 노드 가산점). 순수 벡터DB·순수
그래프DB가 못 하는 영역 = `owl:sameAs` 후보 발견. O-3·R-1·R-2 선결.

**R-3a [P1] WHERE 속성 필터** ✅ 구현 완료 (2026-06-16). 사용자 원래 예시
("네이버 A와 유사한 **쿠팡** 상품")를 완성: `RECOMMEND SIMILAR TO <vid>
(OVER ...|BY EMBEDDING ...) WHERE <predicate> [LIMIT k]`. 양 모드(neighbors,
embedding) 공통. 후보를 점수순 정렬 후, 방출 시 정점 디코드→속성 평탄화
(bare + `{tag}.{prop}` 양형)→기존 `Evaluator::evaluate_condition`로 술어 평가,
통과분만 top-k 방출(`load_candidate_props` + `passes_filter`). 술어 평가 오류는
해당 후보 드롭(쿼리 전체 실패 아님). **WHERE 없으면 정점 디코드 0 (R-1/R-2a
핫패스 불변)**. 회귀: 파서 2(임베딩/neighbors WHERE) + 실행기 2(채널 필터
full-pipeline·neighbors 필터).
**시드 상대 비교 추가 (2026-06-16):** 시드 정점 속성을 `seed` 변수로 바인딩 →
`WHERE channel != seed.channel`("시드와 다른 채널")처럼 값 하드코딩 없이 비교.
`passes_filter`가 current=후보 + variable `seed`=시드 props로 EvalContext 구성.
회귀 1(`embedding_seed_relative_filter_different_channel`).
**caveat (리뷰 F-002):** 공유 evaluator의 bare-식별자 분기가 current에 없으면
변수까지 폴백 → 후보에 *결측*인 prop을 bare로 참조하면 시드 값으로 해석될 수
있음(명시 `seed.prop`는 결정적). evaluator는 GO/MATCH 공용이라 미수정(고위험).
결측 prop을 항상 드롭하려면 single-id 폴백을 current-only로 좁히는 별도 정리 필요.

**R-3b [P2] 클래스 계층 인지 필터 / 재랭킹** 🟡 클래스 필터 + BLEND 완료 (2026-06-17)

**클래스 계층 인지 필터 ✅ (2026-06-17):** `RECOMMEND ... WHERE is_a("animal")` —
후보 정점의 tag가 `animal`이거나 그 **subclass**면 통과. **O-3 클래스 계층만으로
구현**(전체 추론 엔진 O-5 불필요 — transitive subclass면 충분). `load_candidate_props`가
필터에 `is_a`가 있을 때만 후보의 is-a 집합(태그 ∪ O-3 `class_ancestors` 전이 상위)을
계산해 `__isa__`로 주입, evaluator의 신규 `is_a` 함수가 멤버십 검사. 회귀: 실행기 1
(`embedding_isa_filter_matches_subclasses`: dog⊂animal 매칭, cat 제외) + evaluator 5.

**점수 결합 재랭킹 (BLEND) ✅ (2026-06-17):** 사용자 결정 = 가중치를 쿼리 인자로
노출. `RECOMMEND SIMILAR TO <vid> BLEND EMBEDDING <prop> <w_emb> OVER <edges>
<w_struct> [WHERE] [LIMIT k]` → `score = w_emb·max(0,cosine) + w_struct·jaccard`.
두 신호의 합집합 후보(없는 신호=0 기여), cosine은 [0,1] 클램프(음수=비유사)로
0..1 스케일 통일. 결과 컬럼 vid/score/emb/struct. `structural_scores` 헬퍼 추출
(neighbors와 공유), `consume_number`(음수 거부), `RecommendBy::Blend`. WHERE/is_a/
seed 필터 전부 호환. 회귀: 파서 2(blend 파싱·음수 거부) + 실행기 1(가중치별 랭킹 역전).

**미착수 (별도 트랙):**
- **추론 포함 매칭(subclass 초과)**: `inverseOf`/`transitiveProperty`/`sameAs` 등
  RDFS-Plus 추론 기반 필터는 **O-5(추론 엔진) 선결**. is_a(subclass)는 O-3로
  충분하나, 그 이상의 시맨틱 추론은 O-5 영역.

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
