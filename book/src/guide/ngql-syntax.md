# nGQL 문법 가이드

ByoriDB는 그래프 데이터베이스를 위한 SQL과 유사한 쿼리 언어인 nGQL(Graph Query Language)을 사용합니다.

## 개요

nGQL은 세 가지 범주의 문을 지원합니다:

- **DDL (Data Definition Language)**: 스키마와 스페이스를 정의
- **DML (Data Manipulation Language)**: 데이터를 삽입, 수정, 삭제
- **DQL (Data Query Language)**: 그래프를 쿼리하고 순회

## 인증

사용자 이름과 비밀번호로 ByoriDB에 연결합니다:

```
Username: root
Password: value of BYORIDB_ROOT_PASSWORD, or the generated password logged at startup
```

## 빠른 참조

| 범주 | 문 |
|----------|------------|
| DDL | `CREATE SPACE`, `DROP SPACE`, `CREATE TAG`, `ALTER TAG`, `DROP TAG`, `CREATE EDGE`, `ALTER EDGE`, `DROP EDGE` |
| DML | `INSERT VERTEX`, `UPDATE VERTEX`, `DELETE VERTEX`, `INSERT EDGE`, `DELETE EDGE` |
| DQL | `FETCH PROP`, `GO`, `MATCH`, `LOOKUP`, `FIND PATH` |

## 데이터 타입

| 타입 | 설명 | 예시 |
|------|-------------|---------|
| `BOOL` | 불리언 | `true`, `false` |
| `INT8` | 8비트 정수 | `127` |
| `INT16` | 16비트 정수 | `32767` |
| `INT32` | 32비트 정수 | `2147483647` |
| `INT64` | 64비트 정수 | `42` |
| `FLOAT` | 32비트 부동소수점 | `3.14` |
| `DOUBLE` | 64비트 부동소수점 | `3.14159` |
| `STRING` | 가변 길이 텍스트 | `'hello'` |
| `TIMESTAMP` | Unix 타임스탬프 | `1234567890` |
| `DATE` | 날짜 | `2024-01-15` |
| `DATETIME` | 날짜와 시간 | `2024-01-15T10:30:00` |

> **참고:** 정수 타입에는 `INT` 대신 `INT64`를 사용하세요.

## 참고 사항

1. **대소문자 구분**: 키워드는 대소문자를 구분하지 않지만(`CREATE` = `create`), 식별자는 대소문자를 구분합니다.
2. **문자열 따옴표**: 문자열 값에는 작은따옴표를 사용합니다: `'hello'`
3. **세미콜론**: 문 끝에서 선택 사항입니다.
4. **버텍스 ID**: 정수(`INT64`)여야 합니다.
