[English](../../guide/cli.html)

# 명령줄 클라이언트

`byoridb-cli`는 인증형 gRPC 클라이언트입니다. 한 줄 REPL과 단일 쿼리
비대화형 모드를 지원하지만 완전한 관리 셸은 아닙니다.

## 접속

자격 증명은 필수입니다. 비밀번호가 명령줄이나 셸 기록에 남지 않도록 환경변수를
우선 사용하세요.

```bash
export BYORIDB_USER=root
export BYORIDB_PASSWORD='your-root-password'
cargo run --locked --release -p byoridb-client --bin byoridb-cli
```

다른 엔드포인트에 접속하려면 다음과 같이 실행합니다.

```bash
byoridb-cli --addr 192.0.2.10:9669
```

현재 전송 방식은 평문 gRPC입니다. 원격 연결은 사설 네트워크, VPN 또는 신뢰할
수 있는 TLS 터널을 사용하세요.

## 옵션

| 옵션 | 의미 |
| --- | --- |
| `-a, --addr <ADDR>` | gRPC 주소, 기본값 `127.0.0.1:9669` |
| `-u, --user <USER>` | 필수 사용자 이름, `BYORIDB_USER`도 사용 가능 |
| `-p, --password <PASSWORD>` | 필수 비밀번호, `BYORIDB_PASSWORD`도 사용 가능 |
| `-e, --execute <QUERY>` | 쿼리 하나를 실행하고 종료 |
| `-h, --help` | 자동 생성 도움말 표시 |
| `-V, --version` | 클라이언트 버전 표시 |

`--password`로 전달한 값은 셸 기록과 로컬 프로세스 조회에 노출될 수 있습니다.
내장 방식 중에는 `BYORIDB_PASSWORD`가 더 안전합니다.

## REPL 동작

인증 후 프롬프트는 다음과 같습니다.

```text
Connected to byoridb-server at 127.0.0.1:9669
byoridb>
```

한 줄에 하나의 논리적 요청을 입력합니다.

```sql
CREATE SPACE demo;
USE demo;
SHOW TAGS;
```

REPL은 `rustyline` 기반 줄 편집과 위/아래 키 기록을 제공합니다. 종료할 때 현재
작업 디렉터리의 `history.txt`에 입력한 줄을 저장합니다. 이 파일에는
`CREATE USER`나 `ALTER USER`의 비밀번호를 포함한 민감한 쿼리도 들어갈 수
있으므로 적절히 보호하거나 삭제하세요.

내장 종료 단어는 대소문자를 구분하지 않는 `quit`와 `exit`뿐입니다. Ctrl-C와
Ctrl-D도 REPL을 닫습니다. `:help` 같은 콜론 명령, 여러 줄 이어쓰기, 탭 완성은
구현되지 않았습니다. 세미콜론으로 연결한 복합 요청도 한 줄에 넣으세요.

## 결과

데이터셋은 유니코드 표로 렌더링됩니다. 컬럼은 있지만 행이 없는 조회는
`Empty set.`, 데이터셋이 없는 성공 문은 `Executed successfully.`를 출력합니다.
그 밖의 JSON 값은 보기 좋은 JSON으로 출력합니다.

## 쿼리 하나 실행

```bash
BYORIDB_USER=root \
BYORIDB_PASSWORD='your-root-password' \
byoridb-cli --execute 'SHOW SPACES;'
```

연결, 인증, 쿼리 실행 또는 JSON 디코딩이 실패하면 0이 아닌 상태로 종료합니다.

여러 문과 결과를 안정적으로 처리해야 하는 스크립트에는 구조화된 클라이언트
라이브러리나 HTTP API를 사용하세요. CLI에는 네이티브 `--file` 옵션이 없습니다.
