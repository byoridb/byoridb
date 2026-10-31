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

명령은 timestamp·나노초·무작위 suffix가 있는 디렉터리를 배타적으로 생성합니다.
`create` 또는 `list`가 출력한 정확한 ID를 사용하세요. 아래 예시는 계속 지원하는
legacy ID입니다.

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

`verify`는 manifest, 정확한 파일 크기와 format 1 archive의 SHA-256을 확인합니다.
격리된 사본을 열어 redb page 무결성과 모든 table entry를 검사하며 원본 백업과
source 파일은 수정하거나 복구하지 않습니다. 누락·절단·손상·미지원 manifest/data는
전체 검증에서도 nonzero exit로 실패합니다. 버전과 hash가 없는 legacy manifest는
별도로 표시하고 크기·물리 무결성을 검사하지만 과거 digest를 제공하지는 않습니다.
Format 1은 `kv`와 `history`가 모두 필요하며 legacy `kv` 전용 archive는 읽을 수
있습니다. Hash는 손상 검출용으로 인증·암호화를 제공하지 않습니다. 검증은 application
query를 실행하지 않으며 검증·복원 임시 사본을 위한 추가 디스크 공간이 필요합니다.

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

`restore --overwrite`는 교체 데이터를 모두 복사·검증한 뒤 기존 디렉터리를 이동합니다.
일반적인 복사·검증·교체 실패에서는 기존 target을 보존하거나 되돌립니다. 실행 중인
target의 redb lock, 겹치는 backup/target 경로, 상위 경로 탈출과 symlink target은
거부합니다. 두 directory rename 사이 강제 종료는 crash-atomic하지 않습니다.
재시작·재시도 전에 형제 경로 `.byoridb-restore-*/previous`를 보존하고 확인하세요.
OS rollback도 실패하면 복구 파일의 경로를 오류에 표시합니다. 복원 중 다른 복원
프로세스나 서버를 시작하지 마세요.

자동 복구 검증은 실제 CLI로 생성한 archive를 복원한 뒤 current/history/tombstone
쿼리, schema, index, 사용자 인증과 string VID mapping을 비교합니다. RTO는 운영
데이터로 직접 측정한 시간을 기준으로 정하세요.

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
