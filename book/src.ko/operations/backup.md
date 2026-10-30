# 백업 및 복원

[English](../../operations/backup.html) | **한국어**

ByoriDB는 `byoridb-backup`으로 snapshot을 생성하고 복원합니다. redb의 current view와
history table을 별도 `data.redb` 파일에 복사하고 metadata를 함께 기록합니다.

증분 백업, WAL 아카이빙, point-in-time recovery, space별 복원, S3/object-store 직접
출력 및 암호화는 **지원하지 않습니다.** Snapshot 생성이 성공한 뒤 외부 도구로 원격
복사와 암호화를 수행하세요.

## 도구 빌드

```bash
cargo build --locked --release -p byoridb --bin byoridb-backup
export PATH="$PWD/target/release:$PATH"
```

Database 인자는 `data.redb` 파일 자체가 아니라 그 파일을 담은 디렉터리입니다.
`BYORIDB__STORAGE__DATA_PATHS`로 변경하지 않았다면 standalone 기본 경로는
`data/storage`입니다.

## 백업에는 독점 접근이 필요합니다

Standalone server와 backup CLI는 별도 프로세스입니다. redb는 서버가 열어 둔
database를 CLI가 동시에 여는 것을 막으므로 실행 중인 서버의 백업은 현재
`Database already open. Cannot acquire lock.` 오류로 실패합니다.

계획된 offline 작업 시간에 다음 순서로 실행하세요.

1. Traffic을 중단하고 `byoridb-server`를 정상 종료합니다.
2. 마지막 redb checkpoint가 완료되도록 서버의 종료를 확인합니다.
3. Data directory를 대상으로 `byoridb-backup create`를 실행합니다.
4. 새 백업과 아래 검증 항목을 확인합니다.
5. 서버를 다시 시작합니다.

변경 중인 `data.redb`를 `cp`로 복사하지 마세요. 백업 구현은 read transaction을 열고
`kv`와 `history`를 새 redb 파일로 복사합니다.

## Snapshot 생성 및 조회

```bash
byoridb-backup create \
  --db /var/lib/byoridb/data \
  --backup-dir /var/lib/byoridb/backups \
  --label "daily-before-upgrade"
```

명령은 다음과 같이 timestamp 기반 디렉터리를 만듭니다.

```text
/var/lib/byoridb/backups/backup_1785313593/
├── backup_metadata.json
└── data.redb
```

Unix에서 새로 생성하는 backup root의 mode는 `0700`입니다. 파일에는 password hash와
graph property를 포함한 원본 데이터가 들어 있으므로 다른 곳으로 복사한 뒤에도 제한된
소유권과 접근 권한을 유지하세요.

Catalog 목록과 상세 정보를 확인합니다.

```bash
byoridb-backup list --backup-dir /var/lib/byoridb/backups
byoridb-backup list --backup-dir /var/lib/byoridb/backups --format json
byoridb-backup info \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

CLI의 `--no-flush`는 호환성을 위해 남아 있지만 redb 구현은 별도 WAL flush를
사용하지 않으므로 현재 아무 효과가 없습니다. 이 옵션으로 online backup이 가능해지지
않습니다.

## 검증의 범위

```bash
byoridb-backup verify \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

현재 `verify`는 catalog에서 백업을 찾을 수 있는지만 확인합니다. `data.redb`를 열거나
table 전체, application record, row count를 검증하지 않으며 query도 실행하지
않습니다. 실패한 `create`도 timestamp 디렉터리를 남길 수 있으므로 `verify`만으로
사용 가능한 snapshot이라고 판단하지 마세요.

중요한 백업마다 다음을 확인하세요.

- `create`의 성공 exit code
- `backup_metadata.json`과 비어 있지 않은 `data.redb`의 존재
- 새 디렉터리로 복원
- 운영 환경과 다른 port로 복원 경로의 격리된 서버 시작
- 인증 후 대표적인 current 및 `AS OF` query 결과 확인

## 복원

Destination을 사용하는 프로세스를 먼저 종료하세요. 가능하면 새 디렉터리에 복원합니다.

```bash
byoridb-backup restore \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593 \
  --target /var/lib/byoridb/restored-data
```

검증 서버를 시작하기 전에 `BYORIDB__STORAGE__DATA_PATHS`를 복원 경로로 설정합니다.

```bash
export BYORIDB_ROOT_PASSWORD='managed-secret-for-this-environment'
export BYORIDB__STORAGE__DATA_PATHS=/var/lib/byoridb/restored-data
byoridb-server
```

Snapshot 복원이 서버 버전 간 호환성을 보장하지는 않습니다. 일반적인 upgrade/downgrade
또는 storage format 호환성 보장은 없습니다. 원본 build와 설정을 보존하고, 정상 동작하는
database를 교체하기 전에 사본에서 대상 버전을 검증하세요.

Root 비밀번호는 user record에서 복원하지 않습니다. Root는 항상 서버 시작 시의
`BYORIDB_ROOT_PASSWORD`로 정의합니다. 영속적인 non-root user record는 database
snapshot에 포함됩니다.

`restore --overwrite`는 복사 전에 기존 target 디렉터리를 재귀적으로 삭제합니다.
사용 전에 정확한 target을 확인하고 rollback 사본을 보관하세요.

## 보존 명령

특정 백업을 삭제합니다.

```bash
byoridb-backup delete \
  --backup-dir /var/lib/byoridb/backups \
  --backup-id backup_1785313593
```

가장 최근 catalog entry 5개만 보존합니다.

```bash
byoridb-backup cleanup \
  --backup-dir /var/lib/byoridb/backups \
  --keep 5
```

두 명령은 `--force`가 없으면 확인을 요청합니다. 특히 백업 생성이 실패한 뒤에는
자동 정리 전에 목록을 검토하세요.

`scripts/backup.sh`는 `create`, 개수 기반 cleanup, listing을 묶습니다. 서버를
종료하거나 독점 접근을 확보하지는 않으므로 운영자가 이를 조정한 뒤 예약 실행해야
합니다.

## 운영 확인 사항

- Primary data volume과 다른 장애 영역에 백업을 보관하세요.
- 검증된 외부 도구로 저장 및 전송 중인 snapshot을 암호화하세요.
- 명령 exit code와 예상 파일의 존재 및 크기를 모니터링하세요.
- 전체 복원과 대표적인 temporal query를 정기적으로 검증하세요.
- 백업·복원 훈련에서 측정한 환경별 RPO/RTO를 기록하세요. ByoriDB는 공통으로 적용할
  수치를 제시하지 않습니다.
- Snapshot을 읽는 데 필요한 application build와 설정을 보존하고, 운영 데이터를
  교체하기 전에 사본으로 upgrade를 검증하세요.
