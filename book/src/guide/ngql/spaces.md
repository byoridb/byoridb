# 스페이스 관리

**스페이스(Space)**는 그래프 데이터를 위한 논리적 컨테이너입니다(RDBMS의 데이터베이스와 유사).

## CREATE SPACE

```sql
CREATE SPACE <space_name> (vid_type = INT64);
CREATE SPACE my_graph (vid_type = INT64);
CREATE SPACE IF NOT EXISTS my_graph (vid_type = INT64);
```

**매개변수:**
- `vid_type`: 버텍스 ID 타입. 현재 `INT64`를 지원합니다.

## USE SPACE

```sql
USE <space_name>;
USE my_graph;
```

## SHOW SPACES

```sql
SHOW SPACES;
```

## DROP SPACE

```sql
DROP SPACE <space_name>;
DROP SPACE my_graph;
```
