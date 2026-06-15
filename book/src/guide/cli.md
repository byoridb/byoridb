# CLI 사용법

ByoriDB CLI는 쿼리를 실행할 수 있는 대화형 셸을 제공합니다.

## CLI 시작하기

```bash
# Connect to local server
BYORIDB_USER=root BYORIDB_PASSWORD='<root-password>' byoridb-cli

# Connect to remote server
byoridb-cli --addr 192.168.1.100:9669

# With authentication
byoridb-cli --user root --password mypassword
```

## CLI 명령어

### 연결 명령어

| 명령어 | 설명 |
|---------|-------------|
| `:help` | 도움말 메시지 표시 |
| `:quit` 또는 `:exit` | CLI 종료 |
| `:clear` | 화면 지우기 |

### 실행

nGQL 문을 입력하고 Enter를 눌러 실행합니다:

```
(root@localhost:9669) > CREATE SPACE test(vid_type=INT64);
Execution succeeded

(root@localhost:9669) > USE test;
Switched to space `test`

(root@localhost:9669) [test] > SHOW TAGS;
Empty set
```

### 여러 줄 문

긴 쿼리의 경우, 줄 끝에 `\`를 붙여 이어 입력합니다:

```
(root@localhost:9669) [test] > INSERT VERTEX person(name, age) VALUES \
                              > 1:('Alice', 30), \
                              > 2:('Bob', 25);
Execution succeeded
```

### 쿼리 결과

결과는 표 형식으로 표시됩니다:

```
(root@localhost:9669) [test] > FETCH PROP ON person 1;
+----+--------+-----+
| id | name   | age |
+----+--------+-----+
| 1  | Alice  | 30  |
+----+--------+-----+
1 row in set
```

## 세션 관리

### 현재 스페이스

현재 스페이스는 프롬프트에 표시됩니다:

```
(root@localhost:9669) [my_space] >
```

### 스페이스 전환

```sql
USE another_space;
```

## 팁

1. **탭 자동완성**: Tab을 눌러 키워드를 자동완성합니다
2. **히스토리**: 위/아래 화살표로 명령어 히스토리를 탐색합니다
3. **세미콜론**: 문 끝에서 선택 사항입니다
4. **주석**: 한 줄 주석에는 `--`를 사용합니다

```sql
-- This is a comment
CREATE TAG person(name STRING);  -- inline comment
```

## 비대화형 모드

명령줄에서 쿼리를 실행합니다:

```bash
# Single query
byoridb-cli --execute "SHOW SPACES;"

# From file
while IFS= read -r query; do
  byoridb-cli --execute "$query"
done < queries.ngql

# With connection options
byoridb-cli --addr remote.server:9669 --execute "SHOW SPACES;"
```

## 출력

```bash
# Results are printed as tables in interactive mode.
byoridb-cli
```

## 오류 메시지

```
(root@localhost:9669) [test] > SELECT * FROM person;
Error: Syntax error near 'SELECT'

(root@localhost:9669) > USE nonexistent;
Error: Space 'nonexistent' not found
```
