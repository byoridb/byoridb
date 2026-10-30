# Agent 작업 계약 fixture

[English](README.md)

이 EVAL-1 fixture는 첫 AGT-1 discovery 계약과 기존 query 권한 검사를 실행합니다.
결정적인 서비스 테스트이며 모델이 생성한 작업이나 자율 Agent 성공률 측정이 아닙니다.

저장소 루트에서 실행합니다.

```bash
cargo test --locked --test agent_workflow_test -- --test-threads=1
```

각 fixture는 작업, 초기 데이터, 타입이 명시된 기대 응답, 거부할 연산, 최종 그래프,
근거 범위, seed와 반복 횟수를 선언합니다. 매 실행은 새 메모리 DB와 분리된 root/GUEST
세션을 사용합니다. 테스트 자격 증명은 합성 데이터이며 배포 환경에 접속하지 않습니다.
무작위 데이터나 모델을 사용하지 않습니다.

| Fixture | 기대 응답과 최종 상태 |
|---|---|
| `read_int64.json` | Ada와 Grace 두 행. JavaScript의 정확한 정수 범위를 넘는 9007199254740993과 9007199254740995 정수 ID 보존 |
| `read_string.json` | Ada와 Grace 두 행. 외부 문자열 ID `42`와 `사람:grace`를 그대로 보존 |

하네스는 세션의 선택 공간을 바꾸지 않고 스키마를 탐색하고, 전역 역할의 권한 경계와
미지원 capability를 확인하며, 기대한 그래프를 읽습니다. 이어 요청 단위 `read_only`와
GUEST 역할 양쪽에서 금지된 쓰기를 시도하고 최종 person 행이 정확히 유지되는지
확인합니다. GUEST 로그아웃 후에는 query/discovery 접근이 폐기되는지도 검증합니다.
기대 정수는 명시적인 `int64` 태그와 십진 문자열로 기록하고 문자열 ID는 `string` 태그로
구분합니다. 이는 fixture 표현이며 새 query wire encoding을 정의하지 않습니다.

Discovery 메타데이터는 추론 근거가 아닙니다. 이 작업의 답은 asserted 행이며 외부 출처의
사실성을 주장하지 않습니다. 과거 상태, 내부 metadata counter, 모든 그래프 타입을
검증하지는 않습니다. 저장소 오류·손상 metadata·상한·HTTP/gRPC 일치 회귀는 graph와
executor 구현 테스트에 둡니다.

영속 timeout 재시도, revision을 검사하는 동시 쓰기, snapshot 이어 읽기는 미지원으로
기록합니다. 추론·철회와 이후 구현되는 계약의 fixture, 버전이 명시된 모델 실행 절차가
갖춰질 때까지 EVAL-1은 진행 중입니다. 향후 모델 평가는 실행 전에 모델/버전/날짜,
prompt/settings, 데이터셋 버전, 반복 횟수, 호출·토큰·시간 예산을 기록해야 합니다.
동일한 답·근거·최종 상태 검증을 통과한 실행끼리만 비용을 비교합니다. 이 두 fixture는
지연·토큰 절감이나 자율 Agent 성공률을 주장하지 않습니다.
